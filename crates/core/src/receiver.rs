use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use iroh::endpoint::ConnectionType;
use iroh::{Endpoint, NodeId, Watcher as _};
use n0_future::StreamExt;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

use crate::crypto;
use crate::events::{ConnectionPathKind, TransferCompleted, TransferEvent, TransferEventSink};
use crate::progress::transfer_progress_bar;
use crate::protocol::{human_bytes, FileHeader};
use crate::ticket;

/// ALPN protocol identifier — must match the sender.
const ALPN: &[u8] = b"p2p-share/1";

/// ALPN for reverse mode — the connector is the file sender.
const ALPN_REVERSE: &[u8] = b"p2p-share/1-reverse";

type SharedSink = Arc<dyn TransferEventSink>;

fn emit(sink: Option<&SharedSink>, event: TransferEvent) {
    if let Some(sink) = sink {
        sink.on_event(event);
    }
}

fn status(sink: Option<&SharedSink>, msg: impl Into<String>) {
    let msg = msg.into();
    eprintln!("{}", msg);
    emit(sink, TransferEvent::Status(msg));
}

/// Pick a destination path that doesn't collide with existing files.
fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }

    let stem = Path::new(name)
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy();
    let ext = Path::new(name)
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();

    for i in 1u32.. {
        let new_name = format!("{} ({}){}", stem, i, ext);
        let p = dir.join(&new_name);
        if !p.exists() {
            return p;
        }
    }

    unreachable!()
}

/// Spawn a background task that watches connection type changes and prints/emits them.
fn spawn_conn_type_watcher(
    ep: &Endpoint,
    node_id: NodeId,
    sink: Option<SharedSink>,
) -> Option<tokio::task::JoinHandle<()>> {
    let watcher = ep.conn_type(node_id)?;
    let mut stream = watcher.stream_updates_only();
    let handle = tokio::task::spawn(async move {
        while let Some(conn_type) = stream.next().await {
            match &conn_type {
                ConnectionType::Direct(addr) => {
                    eprintln!("Connection upgraded: direct ({})", addr);
                    emit(
                        sink.as_ref(),
                        TransferEvent::ConnectionPath {
                            kind: ConnectionPathKind::Direct(addr.to_string()),
                            latency_ms: None,
                        },
                    );
                }
                ConnectionType::Relay(url) => {
                    eprintln!("Connection changed: relay ({})", url);
                    emit(
                        sink.as_ref(),
                        TransferEvent::ConnectionPath {
                            kind: ConnectionPathKind::Relay(url.to_string()),
                            latency_ms: None,
                        },
                    );
                }
                ConnectionType::Mixed(addr, url) => {
                    eprintln!("Connection changed: mixed (udp: {}, relay: {})", addr, url);
                    emit(
                        sink.as_ref(),
                        TransferEvent::ConnectionPath {
                            kind: ConnectionPathKind::Mixed {
                                udp_addr: addr.to_string(),
                                relay_url: url.to_string(),
                            },
                            latency_ms: None,
                        },
                    );
                }
                ConnectionType::None => {
                    eprintln!("Connection path: none (searching...)");
                    emit(
                        sink.as_ref(),
                        TransferEvent::ConnectionPath {
                            kind: ConnectionPathKind::None,
                            latency_ms: None,
                        },
                    );
                }
            }
        }
    });
    Some(handle)
}

/// Print a summary of the final connection state.
fn print_conn_summary(ep: &Endpoint, node_id: NodeId, sink: Option<&SharedSink>) {
    if let Some(info) = ep.remote_info(node_id) {
        let latency = info.latency.map(|d| d.as_secs_f64() * 1000.0);
        let latency_str = latency
            .map(|d| format!("{:.1}ms", d))
            .unwrap_or_else(|| "unknown".to_string());
        eprintln!(
            "Transfer path: {}, latency: {}",
            info.conn_type, latency_str
        );

        let kind = match info.conn_type {
            ConnectionType::Direct(addr) => ConnectionPathKind::Direct(addr.to_string()),
            ConnectionType::Relay(url) => ConnectionPathKind::Relay(url.to_string()),
            ConnectionType::Mixed(addr, url) => ConnectionPathKind::Mixed {
                udp_addr: addr.to_string(),
                relay_url: url.to_string(),
            },
            ConnectionType::None => ConnectionPathKind::None,
        };
        emit(
            sink,
            TransferEvent::ConnectionPath {
                kind,
                latency_ms: latency,
            },
        );
    }
}

/// Print a QR code to stderr, indented for readability.
fn print_qr(data: &str) {
    if let Ok(qr_string) = qr2term::generate_qr_string(data) {
        for line in qr_string.lines() {
            eprintln!("    {}", line);
        }
    }
}

/// Run the receive side: connect to the sender (via iroh ticket or ip:port),
/// perform the Noise handshake, receive the encrypted file, and verify the checksum.
pub async fn run(target: &str, output_dir: &Path) -> Result<()> {
    run_with_sink(target, output_dir, None).await
}

pub async fn run_with_sink(
    target: &str,
    output_dir: &Path,
    sink: Option<SharedSink>,
) -> Result<()> {
    let target = target.trim();
    if ticket::is_ticket(target) {
        run_iroh(target, output_dir, sink).await
    } else {
        run_direct_tcp(target, output_dir, sink).await
    }
}

/// Connect to the sender via an iroh ticket (NAT-traversal, hole-punching, relay).
async fn run_iroh(target: &str, output_dir: &Path, sink: Option<SharedSink>) -> Result<()> {
    let addr = ticket::deserialize(target)?;

    status(sink.as_ref(), "Connecting to sender via iroh...");

    let ep = Endpoint::builder()
        .bind()
        .await
        .context("failed to create iroh endpoint")?;

    let conn = ep
        .connect(addr, ALPN)
        .await
        .context("failed to connect to sender")?;

    let remote_node_id = conn.remote_node_id()?;
    status(sink.as_ref(), "Connected to sender.");

    if let Some(info) = ep.remote_info(remote_node_id) {
        eprintln!("Connection path: {}", info.conn_type);
    }
    let watcher_handle = spawn_conn_type_watcher(&ep, remote_node_id, sink.clone());

    let (mut send_stream, mut recv_stream) = conn
        .open_bi()
        .await
        .map_err(|e| anyhow::anyhow!("failed to open bi stream: {}", e))?;

    let (mut transport, code) =
        crypto::handshake_initiator(&mut recv_stream, &mut send_stream).await?;
    status(
        sink.as_ref(),
        format!("Encryption established. Verification code: {}", code),
    );
    emit(sink.as_ref(), TransferEvent::HandshakeCode(code));

    receive_file(
        &mut recv_stream,
        &mut send_stream,
        &mut transport,
        output_dir,
        sink.as_ref(),
    )
    .await?;

    if let Some(handle) = watcher_handle {
        handle.abort();
    }
    print_conn_summary(&ep, remote_node_id, sink.as_ref());

    conn.close(0u8.into(), b"done");
    ep.close().await;

    Ok(())
}

/// Connect to the sender via direct TCP (for LAN use when the ip:port is reachable).
async fn run_direct_tcp(addr: &str, output_dir: &Path, sink: Option<SharedSink>) -> Result<()> {
    if addr.contains("p2psh") {
        bail!(
            "This looks like an iroh ticket, not an ip:port address.\n\
             Make sure you are running the latest build of p2p-share on this device."
        );
    }

    status(sink.as_ref(), format!("Connecting to {}...", addr));

    let stream = tokio::net::TcpStream::connect(addr)
        .await
        .with_context(|| format!("failed to connect to {}", addr))?;

    status(sink.as_ref(), "Connected to sender.");

    let (mut reader, mut writer) = stream.into_split();

    let (mut transport, code) = crypto::handshake_initiator(&mut reader, &mut writer).await?;
    status(
        sink.as_ref(),
        format!("Encryption established. Verification code: {}", code),
    );
    emit(sink.as_ref(), TransferEvent::HandshakeCode(code));

    receive_file(
        &mut reader,
        &mut writer,
        &mut transport,
        output_dir,
        sink.as_ref(),
    )
    .await?;

    Ok(())
}

/// Run the receive side in listen mode: create an iroh endpoint, display a
/// QR code / ticket, and wait for a sender to connect with `--to`.
pub async fn run_listen(output_dir: &Path) -> Result<()> {
    run_listen_with_sink(output_dir, None).await
}

pub async fn run_listen_with_sink(output_dir: &Path, sink: Option<SharedSink>) -> Result<()> {
    status(sink.as_ref(), "Setting up secure connection...");

    let ep = Endpoint::builder()
        .alpns(vec![ALPN.to_vec(), ALPN_REVERSE.to_vec()])
        .bind()
        .await
        .context("failed to create iroh endpoint")?;

    status(sink.as_ref(), "Connecting to relay...");
    let relay_timeout =
        tokio::time::timeout(Duration::from_secs(10), ep.home_relay().initialized()).await;

    match &relay_timeout {
        Ok(relay_url) => {
            status(sink.as_ref(), format!("Relay connected: {}", relay_url));
        }
        Err(_) => {
            status(
                sink.as_ref(),
                "Warning: could not connect to relay (timed out).",
            );
            status(sink.as_ref(), "Only direct/LAN connections will work.");
        }
    }

    let node_addr = ep.node_addr().initialized().await;
    let ticket_str = ticket::serialize(&node_addr)?;
    emit(sink.as_ref(), TransferEvent::Ticket(ticket_str.clone()));
    emit(sink.as_ref(), TransferEvent::QrPayload(ticket_str.clone()));

    eprintln!();
    eprintln!("Ready to receive files.");
    eprintln!();
    eprintln!("  Scan this QR code on the sending device:");
    eprintln!();
    print_qr(&ticket_str);
    eprintln!();
    eprintln!("  Or run:\n\n    p2p-share send --to {} <FILE>", ticket_str);
    eprintln!();
    status(sink.as_ref(), "Waiting for sender to connect...");

    let incoming = ep.accept().await.context("no incoming connection")?;

    let conn = incoming
        .accept()
        .map_err(|e| anyhow::anyhow!("failed to accept connection: {}", e))?
        .await
        .map_err(|e| anyhow::anyhow!("connection failed: {}", e))?;

    let remote_node_id = conn.remote_node_id()?;
    status(sink.as_ref(), "Sender connected.");

    if let Some(info) = ep.remote_info(remote_node_id) {
        eprintln!("Connection path: {}", info.conn_type);
    }
    let watcher_handle = spawn_conn_type_watcher(&ep, remote_node_id, sink.clone());

    let (mut send_stream, mut recv_stream) = conn
        .accept_bi()
        .await
        .map_err(|e| anyhow::anyhow!("failed to accept bi stream: {}", e))?;

    let (mut transport, code) =
        crypto::handshake_responder(&mut recv_stream, &mut send_stream).await?;
    status(
        sink.as_ref(),
        format!("Encryption established. Verification code: {}", code),
    );
    emit(sink.as_ref(), TransferEvent::HandshakeCode(code));

    receive_file(
        &mut recv_stream,
        &mut send_stream,
        &mut transport,
        output_dir,
        sink.as_ref(),
    )
    .await?;

    if let Some(handle) = watcher_handle {
        handle.abort();
    }
    print_conn_summary(&ep, remote_node_id, sink.as_ref());

    conn.close(0u8.into(), b"done");
    ep.close().await;

    Ok(())
}

/// Shared file-reception logic used by all receive paths (iroh, TCP, listen).
async fn receive_file<R, W>(
    reader: &mut R,
    writer: &mut W,
    transport: &mut snow::TransportState,
    output_dir: &Path,
    sink: Option<&SharedSink>,
) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let header_bytes = crypto::encrypted_read(reader, transport).await?;
    let header_str = String::from_utf8(header_bytes).context("invalid UTF-8 in file header")?;

    if header_str.is_empty() {
        bail!("Connection closed before receiving file header");
    }

    let header = FileHeader::from_wire(&header_str)?;

    eprintln!();
    status(
        sink,
        format!(
            "Incoming file: {} ({})",
            header.name,
            human_bytes(header.size)
        ),
    );

    crypto::encrypted_write(writer, transport, b"OK\n").await?;

    tokio::fs::create_dir_all(output_dir).await?;
    let dest = unique_path(output_dir, &header.name);
    let temp_dest = unique_path(output_dir, &format!("{}.part", header.name));

    status(sink, format!("Saving to: {}", dest.display()));
    eprintln!();

    let mut file = File::create(&temp_dest).await?;
    let pb = if sink.is_none() {
        Some(transfer_progress_bar(header.size))
    } else {
        None
    };
    let mut received: u64 = 0;
    let mut hasher = blake3::Hasher::new();
    let receive_result: Result<()> = async {
        while received < header.size {
            let plaintext = crypto::encrypted_read(reader, transport).await?;
            if plaintext.is_empty() {
                break;
            }

            file.write_all(&plaintext).await?;
            hasher.update(&plaintext);
            received += plaintext.len() as u64;
            if let Some(pb) = &pb {
                pb.set_position(received);
            }
            emit(
                sink,
                TransferEvent::Progress {
                    done: received,
                    total: header.size,
                },
            );
        }

        file.flush().await?;
        if let Some(pb) = &pb {
            pb.finish_with_message("done");
        }
        drop(file);

        if received != header.size {
            bail!(
                "Incomplete transfer: got {} of {} bytes",
                received,
                header.size
            );
        }

        let computed_hash = hasher.finalize().to_hex().to_string();
        if computed_hash != header.blake3 {
            bail!(
                "Checksum mismatch!\n  expected: {}\n  got:      {}",
                header.blake3,
                computed_hash
            );
        }

        tokio::fs::rename(&temp_dest, &dest).await?;
        crypto::encrypted_write(writer, transport, b"DONE\n").await?;
        Ok(())
    }
    .await;

    if let Err(err) = receive_result {
        let _ = tokio::fs::remove_file(&temp_dest).await;
        return Err(err);
    }

    eprintln!();
    status(
        sink,
        format!(
            "File received successfully: {} ({})",
            dest.display(),
            human_bytes(header.size)
        ),
    );
    status(sink, "Checksum verified (blake3).");
    emit(
        sink,
        TransferEvent::Completed(TransferCompleted {
            file_name: header.name,
            size_bytes: header.size,
            saved_path: Some(dest),
        }),
    );

    Ok(())
}
