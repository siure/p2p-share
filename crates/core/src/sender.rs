use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use iroh::endpoint::ConnectionType;
use iroh::{Endpoint, NodeAddr, NodeId, Watcher as _};
use n0_future::StreamExt;
use tokio::fs::File;
use tokio::io::AsyncReadExt;

use crate::bundle;
use crate::crypto;
use crate::events::{
    ConnectionPathKind, TransferCompleted, TransferContentKind, TransferEvent, TransferEventSink,
};
use crate::progress::transfer_progress_bar;
use crate::protocol::{human_bytes, FileHeader, CHUNK_SIZE};
use crate::ticket;

/// ALPN protocol identifier for p2p-share connections.
const ALPN: &[u8] = b"p2p-share/1";

/// ALPN for reverse mode: the connector is the file sender.
const ALPN_REVERSE: &[u8] = b"p2p-share/1-reverse";

type SharedSink = Arc<dyn TransferEventSink>;

#[derive(Debug, Clone)]
struct PreparedTransfer {
    transfer_path: PathBuf,
    wire_name: String,
    logical_name: String,
    file_size: u64,
    hash: String,
    content_kind: TransferContentKind,
    item_count: u64,
    cleanup_path: Option<PathBuf>,
}

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

async fn hash_file(file_path: &Path, sink: Option<&SharedSink>, label: &str) -> Result<String> {
    status(sink, label);
    let path = file_path.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<String> {
        let mut hasher = blake3::Hasher::new();
        let mut file = std::fs::File::open(&path)?;
        std::io::copy(&mut file, &mut hasher)?;
        Ok(hasher.finalize().to_hex().to_string())
    })
    .await?
}

async fn prepare_send_paths(
    file_paths: &[PathBuf],
    sink: Option<&SharedSink>,
) -> Result<PreparedTransfer> {
    if file_paths.is_empty() {
        bail!("at least one file is required");
    }

    if file_paths.len() == 1 {
        let transfer_path = file_paths[0].clone();
        let metadata = tokio::fs::metadata(&transfer_path)
            .await
            .with_context(|| format!("cannot access {:?}", transfer_path))?;

        if !metadata.is_file() {
            bail!("{:?} is not a regular file", transfer_path);
        }

        let wire_name = transfer_path
            .file_name()
            .context("path has no file name")?
            .to_string_lossy()
            .to_string();
        let hash = hash_file(&transfer_path, sink, "Hashing file...").await?;

        return Ok(PreparedTransfer {
            logical_name: wire_name.clone(),
            transfer_path,
            wire_name,
            file_size: metadata.len(),
            hash,
            content_kind: TransferContentKind::File,
            item_count: 1,
            cleanup_path: None,
        });
    }

    status(
        sink,
        format!("Preparing bundle for {} files...", file_paths.len()),
    );
    let bundle_build = bundle::create_bundle(file_paths).await?;
    let cleanup_path = bundle_build.bundle_path.clone();

    let result: Result<PreparedTransfer> = async {
        let metadata = tokio::fs::metadata(&bundle_build.bundle_path)
            .await
            .with_context(|| format!("cannot access {:?}", bundle_build.bundle_path))?;

        if !metadata.is_file() {
            bail!("{:?} is not a regular file", bundle_build.bundle_path);
        }

        let wire_name = bundle_build
            .bundle_path
            .file_name()
            .context("bundle path has no file name")?
            .to_string_lossy()
            .to_string();
        let hash = hash_file(
            &bundle_build.bundle_path,
            sink,
            "Hashing transfer bundle...",
        )
        .await?;

        Ok(PreparedTransfer {
            transfer_path: bundle_build.bundle_path,
            wire_name,
            logical_name: bundle_build.logical_name,
            file_size: metadata.len(),
            hash,
            content_kind: TransferContentKind::Bundle,
            item_count: bundle_build.item_count,
            cleanup_path: Some(cleanup_path.clone()),
        })
    }
    .await;

    if result.is_err() {
        let _ = tokio::fs::remove_file(&cleanup_path).await;
    }

    result
}

async fn cleanup_temp_file(path: Option<&Path>) {
    if let Some(path) = path {
        let _ = tokio::fs::remove_file(path).await;
    }
}

fn ready_to_send_message(prepared: &PreparedTransfer) -> String {
    match prepared.content_kind {
        TransferContentKind::File => format!(
            "Ready to send: {} ({})",
            prepared.logical_name,
            human_bytes(prepared.file_size)
        ),
        TransferContentKind::Bundle => format!(
            "Ready to send: {} ({} files, {})",
            prepared.logical_name,
            prepared.item_count,
            human_bytes(prepared.file_size)
        ),
    }
}

fn sent_success_message(prepared: &PreparedTransfer) -> String {
    match prepared.content_kind {
        TransferContentKind::File => format!(
            "File sent successfully: {} ({})",
            prepared.logical_name,
            human_bytes(prepared.file_size)
        ),
        TransferContentKind::Bundle => format!(
            "Files sent successfully: {} ({} files, {})",
            prepared.logical_name,
            prepared.item_count,
            human_bytes(prepared.file_size)
        ),
    }
}

/// Send the file over an already-established encrypted channel.
/// Used by both normal and reverse modes.
async fn send_file<R, W>(
    reader: &mut R,
    writer: &mut W,
    transport: &mut snow::TransportState,
    prepared: &PreparedTransfer,
    sink: Option<&SharedSink>,
) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let header = FileHeader {
        name: prepared.wire_name.clone(),
        size: prepared.file_size,
        blake3: prepared.hash.clone(),
        content_kind: Some(prepared.content_kind),
        item_count: Some(prepared.item_count),
        logical_name: (prepared.content_kind == TransferContentKind::Bundle)
            .then(|| prepared.logical_name.clone()),
    };
    let header_bytes = header.to_wire()?;
    crypto::encrypted_write(writer, transport, &header_bytes).await?;

    let ack = crypto::encrypted_read(reader, transport).await?;
    let ack_str = String::from_utf8_lossy(&ack);
    let ack_str = ack_str.trim();

    if ack_str != "OK" {
        bail!("Receiver rejected the transfer: {}", ack_str);
    }

    let transfer_label = if prepared.content_kind == TransferContentKind::Bundle {
        format!(
            "Receiver accepted. Sending {} files...",
            prepared.item_count
        )
    } else {
        "Receiver accepted. Sending file...".to_string()
    };
    status(sink, transfer_label);

    let mut file = File::open(&prepared.transfer_path).await?;
    let pb = if sink.is_none() {
        Some(transfer_progress_bar(prepared.file_size))
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
                total: prepared.file_size,
            },
        );
    }

    if let Some(pb) = pb {
        pb.finish_with_message("done");
    }

    if sent != prepared.file_size {
        bail!(
            "local file changed during transfer: read {} of {} bytes",
            sent,
            prepared.file_size
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
    let file_paths = vec![file_path.to_path_buf()];
    run_paths_with_sink(&file_paths, None).await
}

pub async fn run_paths(file_paths: &[PathBuf]) -> Result<()> {
    run_paths_with_sink(file_paths, None).await
}

pub async fn run_with_sink(file_path: &Path, sink: Option<SharedSink>) -> Result<()> {
    let file_paths = vec![file_path.to_path_buf()];
    run_paths_with_sink(&file_paths, sink).await
}

pub async fn run_paths_with_sink(file_paths: &[PathBuf], sink: Option<SharedSink>) -> Result<()> {
    let prepared = prepare_send_paths(file_paths, sink.as_ref()).await?;
    let cleanup_path = prepared.cleanup_path.clone();

    let result: Result<()> = async {
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
        eprintln!("{}", ready_to_send_message(&prepared));
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
            &prepared,
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
        status(sink.as_ref(), sent_success_message(&prepared));
        emit(
            sink.as_ref(),
            TransferEvent::Completed(TransferCompleted {
                file_name: prepared.logical_name.clone(),
                size_bytes: prepared.file_size,
                saved_path: None,
                content_kind: prepared.content_kind,
                item_count: prepared.item_count,
            }),
        );

        conn.close(0u8.into(), b"done");
        ep.close().await;

        Ok(())
    }
    .await;

    cleanup_temp_file(cleanup_path.as_deref()).await;
    result
}

/// Run the send side (reverse mode): connect to a receiver that is already
/// listening (started with `p2p-share receive --qr`).
pub async fn run_reverse(file_path: &Path, target: &str) -> Result<()> {
    let file_paths = vec![file_path.to_path_buf()];
    run_reverse_paths_with_sink(&file_paths, target, None).await
}

pub async fn run_reverse_paths(file_paths: &[PathBuf], target: &str) -> Result<()> {
    run_reverse_paths_with_sink(file_paths, target, None).await
}

pub async fn run_reverse_with_sink(
    file_path: &Path,
    target: &str,
    sink: Option<SharedSink>,
) -> Result<()> {
    let file_paths = vec![file_path.to_path_buf()];
    run_reverse_paths_with_sink(&file_paths, target, sink).await
}

pub async fn run_reverse_paths_with_sink(
    file_paths: &[PathBuf],
    target: &str,
    sink: Option<SharedSink>,
) -> Result<()> {
    let target = target.trim();
    let prepared = prepare_send_paths(file_paths, sink.as_ref()).await?;
    let cleanup_path = prepared.cleanup_path.clone();

    let result: Result<()> = async {
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
            &prepared,
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
        status(sink.as_ref(), sent_success_message(&prepared));
        emit(
            sink.as_ref(),
            TransferEvent::Completed(TransferCompleted {
                file_name: prepared.logical_name.clone(),
                size_bytes: prepared.file_size,
                saved_path: None,
                content_kind: prepared.content_kind,
                item_count: prepared.item_count,
            }),
        );

        conn.close(0u8.into(), b"done");
        ep.close().await;

        Ok(())
    }
    .await;

    cleanup_temp_file(cleanup_path.as_deref()).await;
    result
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
