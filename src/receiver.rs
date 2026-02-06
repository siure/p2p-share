use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use iroh::endpoint::ConnectionType;
use iroh::{Endpoint, NodeId, Watcher as _};
use n0_future::StreamExt;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;

use crate::crypto;
use crate::progress::transfer_progress_bar;
use crate::protocol::{human_bytes, FileHeader};
use crate::ticket;

/// ALPN protocol identifier — must match the sender.
const ALPN: &[u8] = b"p2p-share/1";

/// ALPN for reverse mode — the connector is the file sender.
const ALPN_REVERSE: &[u8] = b"p2p-share/1-reverse";

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

/// Run the receive side: connect to the sender (via iroh ticket or ip:port),
/// perform the Noise handshake, receive the encrypted file, and verify the checksum.
pub async fn run(target: &str, output_dir: &Path) -> Result<()> {
    let target = target.trim();
    if ticket::is_ticket(target) {
        run_iroh(target, output_dir).await
    } else {
        run_direct_tcp(target, output_dir).await
    }
}

// ─── Normal mode: receiver connects to sender ───────────────────────────────

/// Connect to the sender via an iroh ticket (NAT-traversal, hole-punching, relay).
async fn run_iroh(target: &str, output_dir: &Path) -> Result<()> {
    let addr = ticket::deserialize(target)?;

    eprintln!("Connecting to sender via iroh...");

    let ep = Endpoint::builder()
        .bind()
        .await
        .context("failed to create iroh endpoint")?;

    let conn = ep
        .connect(addr, ALPN)
        .await
        .context("failed to connect to sender")?;

    let remote_node_id = conn.remote_node_id()?;
    eprintln!("Connected to sender.");

    if let Some(info) = ep.remote_info(remote_node_id) {
        eprintln!("Connection path: {}", info.conn_type);
    }
    let watcher_handle = spawn_conn_type_watcher(&ep, remote_node_id);

    // We (the QUIC connector) open a bi-stream and are the Noise initiator.
    let (mut send_stream, mut recv_stream) = conn
        .open_bi()
        .await
        .map_err(|e| anyhow::anyhow!("failed to open bi stream: {}", e))?;

    let (mut transport, code) =
        crypto::handshake_initiator(&mut recv_stream, &mut send_stream).await?;
    eprintln!("Encryption established. Verification code: {}", code);

    receive_file(&mut recv_stream, &mut send_stream, &mut transport, output_dir).await?;

    if let Some(handle) = watcher_handle {
        handle.abort();
    }
    print_conn_summary(&ep, remote_node_id);

    conn.close(0u8.into(), b"done");
    ep.close().await;

    Ok(())
}

/// Connect to the sender via direct TCP (for LAN use when the ip:port is reachable).
async fn run_direct_tcp(addr: &str, output_dir: &Path) -> Result<()> {
    if addr.contains("p2psh") {
        bail!(
            "This looks like an iroh ticket, not an ip:port address.\n\
             Make sure you are running the latest build of p2p-share on this device."
        );
    }

    eprintln!("Connecting to {}...", addr);

    let stream = tokio::net::TcpStream::connect(addr)
        .await
        .with_context(|| format!("failed to connect to {}", addr))?;

    eprintln!("Connected to sender.");

    let (mut reader, mut writer) = stream.into_split();

    let (mut transport, code) = crypto::handshake_initiator(&mut reader, &mut writer).await?;
    eprintln!("Encryption established. Verification code: {}", code);

    receive_file(&mut reader, &mut writer, &mut transport, output_dir).await?;

    Ok(())
}

// ─── Listen mode (--qr): receiver creates endpoint, waits for sender ────────

/// Run the receive side in listen mode: create an iroh endpoint, display a
/// QR code / ticket, and wait for a sender to connect with `--to`.
pub async fn run_listen(output_dir: &Path) -> Result<()> {
    eprintln!("Setting up secure connection...");

    let ep = Endpoint::builder()
        .alpns(vec![ALPN.to_vec(), ALPN_REVERSE.to_vec()])
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
    eprintln!("Ready to receive files.");
    eprintln!();
    eprintln!("  Scan this QR code on the sending device:");
    eprintln!();
    print_qr(&ticket_str);
    eprintln!();
    eprintln!(
        "  Or run:\n\n    p2p-share send --to {} <FILE>",
        ticket_str
    );
    eprintln!();
    eprintln!("Waiting for sender to connect...");

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
    eprintln!("Sender connected.");

    if let Some(info) = ep.remote_info(remote_node_id) {
        eprintln!("Connection path: {}", info.conn_type);
    }
    let watcher_handle = spawn_conn_type_watcher(&ep, remote_node_id);

    // In reverse mode the sender (QUIC connector) opens the bi-stream and is the
    // Noise initiator.  We accept the stream and are the Noise responder.
    let (mut send_stream, mut recv_stream) = conn
        .accept_bi()
        .await
        .map_err(|e| anyhow::anyhow!("failed to accept bi stream: {}", e))?;

    let (mut transport, code) =
        crypto::handshake_responder(&mut recv_stream, &mut send_stream).await?;
    eprintln!("Encryption established. Verification code: {}", code);

    // --- Receive file using the shared logic ---
    receive_file(&mut recv_stream, &mut send_stream, &mut transport, output_dir).await?;

    if let Some(handle) = watcher_handle {
        handle.abort();
    }
    print_conn_summary(&ep, remote_node_id);

    conn.close(0u8.into(), b"done");
    ep.close().await;

    Ok(())
}

// ─── Shared file reception logic ────────────────────────────────────────────

/// Shared file-reception logic used by all receive paths (iroh, TCP, listen).
async fn receive_file<R, W>(
    reader: &mut R,
    writer: &mut W,
    transport: &mut snow::TransportState,
    output_dir: &Path,
) -> Result<()>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    // --- Receive header (encrypted) ---
    let header_bytes = crypto::encrypted_read(reader, transport).await?;
    let header_str =
        String::from_utf8(header_bytes).context("invalid UTF-8 in file header")?;

    if header_str.is_empty() {
        bail!("Connection closed before receiving file header");
    }

    let header = FileHeader::from_wire(&header_str)?;

    eprintln!();
    eprintln!(
        "Incoming file: {} ({})",
        header.name,
        human_bytes(header.size)
    );

    // --- Send ACK (encrypted) ---
    crypto::encrypted_write(writer, transport, b"OK\n").await?;

    // --- Prepare output file ---
    tokio::fs::create_dir_all(output_dir).await?;
    let dest = unique_path(output_dir, &header.name);

    eprintln!("Saving to: {}", dest.display());
    eprintln!();

    let mut file = File::create(&dest).await?;
    let pb = transfer_progress_bar(header.size);
    let mut received: u64 = 0;
    let mut hasher = blake3::Hasher::new();

    // --- Receive file data (encrypted) ---
    while received < header.size {
        let plaintext = crypto::encrypted_read(reader, transport).await?;
        if plaintext.is_empty() {
            break;
        }

        file.write_all(&plaintext).await?;
        hasher.update(&plaintext);
        received += plaintext.len() as u64;
        pb.set_position(received);
    }

    file.flush().await?;
    pb.finish_with_message("done");

    // --- Verify ---
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

    // Tell the sender we got everything successfully.
    crypto::encrypted_write(writer, transport, b"DONE\n").await?;

    eprintln!();
    eprintln!(
        "File received successfully: {} ({})",
        dest.display(),
        human_bytes(header.size)
    );
    eprintln!("Checksum verified (blake3).");

    Ok(())
}
