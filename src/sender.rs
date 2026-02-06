use std::path::Path;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use iroh::endpoint::ConnectionType;
use iroh::{Endpoint, NodeId, Watcher as _};
use n0_future::StreamExt;
use tokio::fs::File;
use tokio::io::AsyncReadExt;

use crate::crypto;
use crate::progress::transfer_progress_bar;
use crate::protocol::{human_bytes, FileHeader, CHUNK_SIZE};
use crate::ticket;

/// ALPN protocol identifier for p2p-share connections.
const ALPN: &[u8] = b"p2p-share/1";

/// ALPN for reverse mode: the connector is the file sender.
const ALPN_REVERSE: &[u8] = b"p2p-share/1-reverse";

/// Spawn a background task that watches connection type changes and prints them.
fn spawn_conn_type_watcher(ep: &Endpoint, node_id: NodeId) -> Option<tokio::task::JoinHandle<()>> {
    let watcher = ep.conn_type(node_id)?;
    let mut stream = watcher.stream_updates_only();
    let handle = tokio::task::spawn(async move {
        while let Some(conn_type) = stream.next().await {
            match &conn_type {
                ConnectionType::Direct(addr) => {
                    eprintln!("Connection upgraded: direct ({})", addr);
                }
                ConnectionType::Relay(url) => {
                    eprintln!("Connection changed: relay ({})", url);
                }
                ConnectionType::Mixed(addr, url) => {
                    eprintln!("Connection changed: mixed (udp: {}, relay: {})", addr, url);
                }
                ConnectionType::None => {
                    eprintln!("Connection path: none (searching...)");
                }
            }
        }
    });
    Some(handle)
}

/// Print a summary of the final connection state.
fn print_conn_summary(ep: &Endpoint, node_id: NodeId) {
    if let Some(info) = ep.remote_info(node_id) {
        let latency_str = info
            .latency
            .map(|d| format!("{:.1}ms", d.as_secs_f64() * 1000.0))
            .unwrap_or_else(|| "unknown".to_string());
        eprintln!("Transfer path: {}, latency: {}", info.conn_type, latency_str);
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
async fn prepare_file(file_path: &Path) -> Result<(String, u64, String)> {
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

    eprintln!("Hashing file...");
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
) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    // --- Send header (encrypted) -------------------------------------------------
    let header = FileHeader {
        name: file_name.to_string(),
        size: file_size,
        blake3: hash.to_string(),
    };
    let header_bytes = header.to_wire()?;
    crypto::encrypted_write(writer, transport, &header_bytes).await?;

    // --- Wait for ACK (encrypted) ------------------------------------------------
    let ack = crypto::encrypted_read(reader, transport).await?;
    let ack_str = String::from_utf8_lossy(&ack);
    let ack_str = ack_str.trim();

    if ack_str != "OK" {
        bail!("Receiver rejected the transfer: {}", ack_str);
    }

    eprintln!("Receiver accepted. Sending file...");

    // --- Stream file data (encrypted) --------------------------------------------
    let mut file = File::open(file_path).await?;
    let pb = transfer_progress_bar(file_size);
    let mut buf = vec![0u8; CHUNK_SIZE];
    let mut sent: u64 = 0;

    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        crypto::encrypted_write(writer, transport, &buf[..n]).await?;
        sent += n as u64;
        pb.set_position(sent);
    }

    pb.finish_with_message("done");

    Ok(())
}

/// Wait for the receiver's "DONE" acknowledgement after finishing the stream.
async fn wait_for_done<R>(
    reader: &mut R,
    transport: &mut snow::TransportState,
) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
{
    match crypto::encrypted_read(reader, transport).await {
        Ok(done) => {
            let done_str = String::from_utf8_lossy(&done);
            if done_str.trim() != "DONE" {
                bail!(
                    "unexpected final message from receiver: {}",
                    done_str.trim()
                );
            }
        }
        Err(_) => {
            // Connection closed before we could read — receiver likely finished fine.
        }
    }
    Ok(())
}

// ─── Normal mode ────────────────────────────────────────────────────────────
// Sender creates endpoint, waits for receiver to connect.

/// Run the send side (normal mode): create an iroh endpoint, wait for a
/// receiver to connect, perform the Noise handshake, then stream the file.
pub async fn run(file_path: &Path) -> Result<()> {
    let (file_name, file_size, hash) = prepare_file(file_path).await?;

    // --- Create iroh endpoint ----------------------------------------------------
    eprintln!("Setting up secure connection...");
    let ep = Endpoint::builder()
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .context("failed to create iroh endpoint")?;

    // Wait for relay — essential for cross-network transfers.
    eprintln!("Connecting to relay...");
    let relay_timeout = tokio::time::timeout(
        Duration::from_secs(10),
        ep.home_relay().initialized(),
    )
    .await;

    match &relay_timeout {
        Ok(relay_url) => {
            eprintln!("Relay connected: {}", relay_url);
        }
        Err(_) => {
            eprintln!("Warning: could not connect to relay (timed out).");
            eprintln!("Only direct/LAN connections will work.");
        }
    }

    let node_addr = ep.node_addr().initialized().await;
    let ticket_str = ticket::serialize(&node_addr)?;

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
    eprintln!("Waiting for receiver to connect...");

    // --- Accept one connection ---------------------------------------------------
    let incoming = ep
        .accept()
        .await
        .context("no incoming connection")?;

    let conn = incoming
        .accept()
        .map_err(|e| anyhow::anyhow!("failed to accept connection: {}", e))?
        .await
        .map_err(|e| anyhow::anyhow!("connection failed: {}", e))?;

    let remote_node_id = conn.remote_node_id()?;
    eprintln!("Receiver connected.");

    if let Some(info) = ep.remote_info(remote_node_id) {
        eprintln!("Connection path: {}", info.conn_type);
    }
    let watcher_handle = spawn_conn_type_watcher(&ep, remote_node_id);

    // Receiver opens bi-stream and is Noise initiator; we are responder.
    let (mut send_stream, mut recv_stream) = conn
        .accept_bi()
        .await
        .map_err(|e| anyhow::anyhow!("failed to accept bi stream: {}", e))?;

    let (mut transport, code) =
        crypto::handshake_responder(&mut recv_stream, &mut send_stream).await?;
    eprintln!("Encryption established. Verification code: {}", code);

    // --- Send file ---------------------------------------------------------------
    send_file(
        &mut recv_stream,
        &mut send_stream,
        &mut transport,
        file_path,
        &file_name,
        file_size,
        &hash,
    )
    .await?;

    // Signal end of data.
    send_stream
        .finish()
        .map_err(|e| anyhow::anyhow!("failed to finish stream: {}", e))?;

    wait_for_done(&mut recv_stream, &mut transport).await?;

    if let Some(handle) = watcher_handle {
        handle.abort();
    }

    eprintln!();
    print_conn_summary(&ep, remote_node_id);
    eprintln!(
        "File sent successfully: {} ({})",
        file_name,
        human_bytes(file_size)
    );

    conn.close(0u8.into(), b"done");
    ep.close().await;

    Ok(())
}

// ─── Reverse mode ───────────────────────────────────────────────────────────
// Sender connects to a waiting receiver's endpoint.  Used with
// `p2p-share send --to <ticket> file.txt` on the phone side.

/// Run the send side (reverse mode): connect to a receiver that is already
/// listening (started with `p2p-share receive --qr`).
pub async fn run_reverse(file_path: &Path, target: &str) -> Result<()> {
    let target = target.trim();
    let (file_name, file_size, hash) = prepare_file(file_path).await?;

    let addr = ticket::deserialize(target)?;

    eprintln!("Connecting to receiver...");

    let ep = Endpoint::builder()
        .bind()
        .await
        .context("failed to create iroh endpoint")?;

    let conn = ep
        .connect(addr, ALPN_REVERSE)
        .await
        .context("failed to connect to receiver")?;

    let remote_node_id = conn.remote_node_id()?;
    eprintln!("Connected to receiver.");

    if let Some(info) = ep.remote_info(remote_node_id) {
        eprintln!("Connection path: {}", info.conn_type);
    }
    let watcher_handle = spawn_conn_type_watcher(&ep, remote_node_id);

    // We (the QUIC connector) open the bi-stream and are the Noise initiator.
    let (mut send_stream, mut recv_stream) = conn
        .open_bi()
        .await
        .map_err(|e| anyhow::anyhow!("failed to open bi stream: {}", e))?;

    let (mut transport, code) =
        crypto::handshake_initiator(&mut recv_stream, &mut send_stream).await?;
    eprintln!("Encryption established. Verification code: {}", code);

    // --- Send file ---------------------------------------------------------------
    send_file(
        &mut recv_stream,
        &mut send_stream,
        &mut transport,
        file_path,
        &file_name,
        file_size,
        &hash,
    )
    .await?;

    // Signal end of data.
    send_stream
        .finish()
        .map_err(|e| anyhow::anyhow!("failed to finish stream: {}", e))?;

    wait_for_done(&mut recv_stream, &mut transport).await?;

    if let Some(handle) = watcher_handle {
        handle.abort();
    }

    eprintln!();
    print_conn_summary(&ep, remote_node_id);
    eprintln!(
        "File sent successfully: {} ({})",
        file_name,
        human_bytes(file_size)
    );

    conn.close(0u8.into(), b"done");
    ep.close().await;

    Ok(())
}
