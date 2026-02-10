use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use iroh::endpoint::ConnectionType;
use iroh::{Endpoint, NodeAddr, NodeId, Watcher as _};
use n0_future::StreamExt;
use tokio::fs::File;
use tokio::io::AsyncReadExt;

use crate::crypto;
use crate::events::{ConnectionPathKind, TransferCompleted, TransferEvent, TransferEventSink};
use crate::progress::transfer_progress_bar;
use crate::protocol::{human_bytes, FileHeader, CHUNK_SIZE};
use crate::ticket;

/// ALPN protocol identifier for p2p-share connections.
const ALPN: &[u8] = b"p2p-share/1";

/// ALPN for reverse mode: the connector is the file sender.
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

/// Print/emit a summary of the final connection state.
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

/// Validate file and return (file_name, file_size, blake3_hash).
async fn prepare_file(
    file_path: &Path,
    sink: Option<&SharedSink>,
) -> Result<(String, u64, String)> {
    let metadata = tokio::fs::metadata(file_path)
        .await
        .with_context(|| format!("cannot access {:?}", file_path))?;

    if !metadata.is_file() {
        bail!("{:?} is not a regular file", file_path);
    }

    let file_name = file_path
        .file_name()
        .context("path has no file name")?
        .to_string_lossy()
        .to_string();

    let file_size = metadata.len();

    status(sink, "Hashing file...");
    let hash = {
        let path = file_path.to_path_buf();
        tokio::task::spawn_blocking(move || -> Result<String> {
            let mut hasher = blake3::Hasher::new();
            let mut file = std::fs::File::open(&path)?;
            std::io::copy(&mut file, &mut hasher)?;
            Ok(hasher.finalize().to_hex().to_string())
        })
        .await??
    };

    Ok((file_name, file_size, hash))
}

/// Send the file over an already-established encrypted channel.
/// Used by both normal and reverse modes.
async fn send_file<R, W>(
    reader: &mut R,
    writer: &mut W,
    transport: &mut snow::TransportState,
    file_path: &Path,
    file_name: &str,
    file_size: u64,
    hash: &str,
    sink: Option<&SharedSink>,
) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let header = FileHeader {
        name: file_name.to_string(),
        size: file_size,
        blake3: hash.to_string(),
    };
    let header_bytes = header.to_wire()?;
    crypto::encrypted_write(writer, transport, &header_bytes).await?;

    let ack = crypto::encrypted_read(reader, transport).await?;
    let ack_str = String::from_utf8_lossy(&ack);
    let ack_str = ack_str.trim();

    if ack_str != "OK" {
        bail!("Receiver rejected the transfer: {}", ack_str);
    }

    status(sink, "Receiver accepted. Sending file...");

    let mut file = File::open(file_path).await?;
    let pb = if sink.is_none() {
        Some(transfer_progress_bar(file_size))
    } else {
        None
    };
    let mut buf = vec![0u8; CHUNK_SIZE];
    let mut sent: u64 = 0;

    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        crypto::encrypted_write(writer, transport, &buf[..n]).await?;
        sent += n as u64;
        if let Some(pb) = &pb {
            pb.set_position(sent);
        }
        emit(
            sink,
            TransferEvent::Progress {
                done: sent,
                total: file_size,
            },
        );
    }

    if let Some(pb) = pb {
        pb.finish_with_message("done");
    }

    if sent != file_size {
        bail!(
            "local file changed during transfer: read {} of {} bytes",
            sent,
            file_size
        );
    }

    Ok(())
}

/// Wait for the receiver's "DONE" acknowledgement after finishing the stream.
async fn wait_for_done<R>(reader: &mut R, transport: &mut snow::TransportState) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
{
    let done = crypto::encrypted_read(reader, transport)
        .await
        .context("connection lost before receiver confirmation")?;
    let done_str = String::from_utf8_lossy(&done);
    if done_str.trim() != "DONE" {
        bail!(
            "unexpected final message from receiver: {}",
            done_str.trim()
        );
    }
    Ok(())
}

/// Run the send side (normal mode): create an iroh endpoint, wait for a
/// receiver to connect, perform the Noise handshake, then stream the file.
pub async fn run(file_path: &Path) -> Result<()> {
    run_with_sink(file_path, None).await
}

pub async fn run_with_sink(file_path: &Path, sink: Option<SharedSink>) -> Result<()> {
    let (file_name, file_size, hash) = prepare_file(file_path, sink.as_ref()).await?;

    status(sink.as_ref(), "Setting up secure connection...");
    let ep = Endpoint::builder()
        .alpns(vec![ALPN.to_vec()])
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
    let advertised_addr = if sink.is_some() && node_addr.relay_url.is_some() {
        status(
            sink.as_ref(),
            "Mobile stability mode: advertising relay-only ticket.",
        );
        NodeAddr::from_parts(node_addr.node_id, node_addr.relay_url.clone(), Vec::new())
    } else {
        node_addr
    };
    let ticket_str = ticket::serialize(&advertised_addr)?;
    emit(sink.as_ref(), TransferEvent::Ticket(ticket_str.clone()));
    emit(sink.as_ref(), TransferEvent::QrPayload(ticket_str.clone()));

    eprintln!();
    eprintln!("Ready to send: {} ({})", file_name, human_bytes(file_size));
    eprintln!();
    eprintln!(
        "  On the receiving device, run:\n\n    p2p-share receive {}",
        ticket_str
    );
    eprintln!();
    eprintln!("  Or scan this QR code:");
    eprintln!();
    print_qr(&ticket_str);
    eprintln!();
    status(sink.as_ref(), "Waiting for receiver to connect...");

    let incoming = ep.accept().await.context("no incoming connection")?;

    let conn = incoming
        .accept()
        .map_err(|e| anyhow::anyhow!("failed to accept connection: {}", e))?
        .await
        .map_err(|e| anyhow::anyhow!("connection failed: {}", e))?;

    let remote_node_id = conn.remote_node_id()?;
    status(sink.as_ref(), "Receiver connected.");

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

    send_file(
        &mut recv_stream,
        &mut send_stream,
        &mut transport,
        file_path,
        &file_name,
        file_size,
        &hash,
        sink.as_ref(),
    )
    .await?;

    send_stream
        .finish()
        .map_err(|e| anyhow::anyhow!("failed to finish stream: {}", e))?;

    wait_for_done(&mut recv_stream, &mut transport).await?;

    if let Some(handle) = watcher_handle {
        handle.abort();
    }

    eprintln!();
    print_conn_summary(&ep, remote_node_id, sink.as_ref());
    status(
        sink.as_ref(),
        format!(
            "File sent successfully: {} ({})",
            file_name,
            human_bytes(file_size)
        ),
    );
    emit(
        sink.as_ref(),
        TransferEvent::Completed(TransferCompleted {
            file_name,
            size_bytes: file_size,
            saved_path: None,
        }),
    );

    conn.close(0u8.into(), b"done");
    ep.close().await;

    Ok(())
}

/// Run the send side (reverse mode): connect to a receiver that is already
/// listening (started with `p2p-share receive --qr`).
pub async fn run_reverse(file_path: &Path, target: &str) -> Result<()> {
    run_reverse_with_sink(file_path, target, None).await
}

pub async fn run_reverse_with_sink(
    file_path: &Path,
    target: &str,
    sink: Option<SharedSink>,
) -> Result<()> {
    let target = target.trim();
    let (file_name, file_size, hash) = prepare_file(file_path, sink.as_ref()).await?;

    let addr = ticket::deserialize(target)?;
    let addr_fallback = addr.clone();

    status(sink.as_ref(), "Connecting to receiver...");

    let ep = Endpoint::builder()
        .bind()
        .await
        .context("failed to create iroh endpoint")?;

    // On mobile, prefer relay-first to avoid unstable direct-path upgrades on
    // some LAN/IPv6 combinations. Fall back to the full address list.
    let conn = if sink.is_some() {
        connect_reverse_relay_first(&ep, addr, addr_fallback, sink.as_ref()).await?
    } else {
        ep.connect(addr_fallback, ALPN_REVERSE)
            .await
            .context("failed to connect to receiver")?
    };

    let remote_node_id = conn.remote_node_id()?;
    status(sink.as_ref(), "Connected to receiver.");

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

    send_file(
        &mut recv_stream,
        &mut send_stream,
        &mut transport,
        file_path,
        &file_name,
        file_size,
        &hash,
        sink.as_ref(),
    )
    .await?;

    send_stream
        .finish()
        .map_err(|e| anyhow::anyhow!("failed to finish stream: {}", e))?;

    wait_for_done(&mut recv_stream, &mut transport).await?;

    if let Some(handle) = watcher_handle {
        handle.abort();
    }

    eprintln!();
    print_conn_summary(&ep, remote_node_id, sink.as_ref());
    status(
        sink.as_ref(),
        format!(
            "File sent successfully: {} ({})",
            file_name,
            human_bytes(file_size)
        ),
    );
    emit(
        sink.as_ref(),
        TransferEvent::Completed(TransferCompleted {
            file_name,
            size_bytes: file_size,
            saved_path: None,
        }),
    );

    conn.close(0u8.into(), b"done");
    ep.close().await;

    Ok(())
}

async fn connect_reverse_relay_first(
    ep: &Endpoint,
    relay_candidate: NodeAddr,
    fallback: NodeAddr,
    sink: Option<&SharedSink>,
) -> Result<iroh::endpoint::Connection> {
    if relay_candidate.relay_url.is_none() {
        return ep
            .connect(fallback, ALPN_REVERSE)
            .await
            .context("failed to connect to receiver");
    }

    let relay_only = NodeAddr::from_parts(
        relay_candidate.node_id,
        relay_candidate.relay_url.clone(),
        Vec::new(),
    );

    status(
        sink,
        "Trying relay-preferred connect (mobile stability mode)...",
    );
    match ep.connect(relay_only, ALPN_REVERSE).await {
        Ok(conn) => Ok(conn),
        Err(err) => {
            status(
                sink,
                format!(
                    "Relay-preferred connect failed ({}). Falling back to direct+relay ticket.",
                    err
                ),
            );
            ep.connect(fallback, ALPN_REVERSE)
                .await
                .context("failed to connect to receiver")
        }
    }
}
