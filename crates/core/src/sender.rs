use anyhow::{bail, Context, Result};
use iroh::Endpoint;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::fs::File;
use tokio::io::AsyncReadExt;

use crate::bundle;
use crate::crypto;
use crate::events::{TransferCompleted, TransferContentKind, TransferEvent};
use crate::progress::transfer_progress_bar;
use crate::protocol::{human_bytes, FileHeader, CHUNK_SIZE};
use crate::ticket;

use crate::engine::{
    self, bounded, emit, status, Session, SharedSink, TransferEngine, ALPN, ALPN_REVERSE,
};

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

async fn hash_file(file_path: &Path, sink: Option<&SharedSink>, label: &str) -> Result<String> {
    status(sink, label);
    let path = file_path.to_path_buf();
    let canceled = Arc::new(AtomicBool::new(false));
    let _cancel = CancelHash(canceled.clone());
    tokio::task::spawn_blocking(move || -> Result<String> {
        let mut hasher = blake3::Hasher::new();
        let mut file = std::fs::File::open(&path)?;
        let mut buffer = vec![0; 256 * 1024];
        loop {
            if canceled.load(Ordering::Relaxed) {
                bail!("file preparation canceled");
            }
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
        }
        Ok(hasher.finalize().to_hex().to_string())
    })
    .await?
}

struct CancelHash(Arc<AtomicBool>);
impl Drop for CancelHash {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
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
        let hash = bundle_build.hash.clone();
        let logical_name = bundle_build.logical_name.clone();
        let item_count = bundle_build.item_count;
        let transfer_path = bundle_build.into_path();

        Ok(PreparedTransfer {
            transfer_path,
            wire_name,
            logical_name,
            file_size: metadata.len(),
            hash,
            content_kind: TransferContentKind::Bundle,
            item_count,
            cleanup_path: Some(cleanup_path.clone()),
        })
    }
    .await;

    if result.is_err() {
        let _ = tokio::fs::remove_file(&cleanup_path).await;
    }

    result
}

impl Drop for PreparedTransfer {
    fn drop(&mut self) {
        if let Some(path) = &self.cleanup_path {
            let _ = std::fs::remove_file(path);
        }
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
async fn send_file<W>(
    writer: &mut W,
    transport: &mut snow::TransportState,
    prepared: &PreparedTransfer,
    sink: Option<&SharedSink>,
) -> Result<()>
where
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
    bounded(
        "sending file metadata",
        crypto::encrypted_write(writer, transport, &header_bytes),
    )
    .await?;

    let transfer_label = if prepared.content_kind == TransferContentKind::Bundle {
        format!("Sending {} files...", prepared.item_count)
    } else {
        "Sending file...".to_string()
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
    let mut ciphertext = Vec::with_capacity(CHUNK_SIZE + 16);
    let mut progress = crate::progress::ProgressThrottle::default();

    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        bounded(
            "sending file data",
            crypto::encrypted_write_with_buffer(writer, transport, &buf[..n], &mut ciphertext),
        )
        .await?;
        sent += n as u64;
        if let Some(pb) = &pb {
            pb.set_position(sent);
        }
        if progress.should_emit(sent, prepared.file_size) {
            emit(
                sink,
                TransferEvent::Progress {
                    done: sent,
                    total: prepared.file_size,
                },
            );
        }
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
    // Finalization includes verified bundle extraction on the receiver's disk,
    // so allow longer than the network's 30-second per-frame stall deadline.
    let done = tokio::time::timeout(
        std::time::Duration::from_secs(5 * 60),
        crypto::encrypted_read(reader, transport),
    )
    .await
    .context("Timed out after 5 minutes waiting for the receiver to save and verify the transfer")?
    .context("connection lost before receiver confirmation")?;
    let done_str = String::from_utf8_lossy(&done);
    if done_str.trim() != "DONE" {
        bail!(
            "unexpected final message from receiver: {}",
            done_str.trim()
        );
    }
    // QUIC's receive stream sends STOP_SENDING(0) when dropped before EOF.
    // Consume the receiver's FIN as well as DONE, so a successful sender cannot
    // accidentally reset the confirmation stream while the peer awaits its ACK.
    let mut extra = [0u8; 1];
    let trailing = bounded(
        "waiting for receiver confirmation stream to finish",
        reader.read(&mut extra),
    )
    .await?;
    if trailing != 0 {
        bail!("unexpected data after receiver confirmation");
    }
    Ok(())
}

/// One-shot CLI/API helpers. Long-lived applications should reuse TransferEngine.
pub async fn run(file_path: &Path) -> Result<()> {
    run_with_sink(file_path, None).await
}
pub async fn run_paths(file_paths: &[PathBuf]) -> Result<()> {
    run_paths_with_sink(file_paths, None).await
}
pub async fn run_with_sink(file_path: &Path, sink: Option<SharedSink>) -> Result<()> {
    run_paths_with_sink(&[file_path.to_path_buf()], sink).await
}
pub async fn run_paths_with_sink(file_paths: &[PathBuf], sink: Option<SharedSink>) -> Result<()> {
    let engine = TransferEngine::bind().await?;
    let result = engine.send_paths(file_paths, None, sink).await;
    engine.close().await;
    result
}
pub async fn run_reverse(file_path: &Path, target: &str) -> Result<()> {
    run_reverse_with_sink(file_path, target, None).await
}
pub async fn run_reverse_paths(file_paths: &[PathBuf], target: &str) -> Result<()> {
    run_reverse_paths_with_sink(file_paths, target, None).await
}
pub async fn run_reverse_with_sink(
    file_path: &Path,
    target: &str,
    sink: Option<SharedSink>,
) -> Result<()> {
    run_reverse_paths_with_sink(&[file_path.to_path_buf()], target, sink).await
}
pub async fn run_reverse_paths_with_sink(
    file_paths: &[PathBuf],
    target: &str,
    sink: Option<SharedSink>,
) -> Result<()> {
    let engine = TransferEngine::bind().await?;
    let result = engine.send_paths(file_paths, Some(target), sink).await;
    engine.close().await;
    result
}

pub(crate) async fn send_on_endpoint(
    ep: &Endpoint,
    file_paths: &[PathBuf],
    target: Option<&str>,
    sink: Option<SharedSink>,
) -> Result<()> {
    let target_addr = target
        .map(|t| ticket::connection_target(t.trim(), ALPN_REVERSE))
        .transpose()?;
    // The shared endpoint is already probing paths while disk preparation runs.
    let prepared = prepare_send_paths(file_paths, sink.as_ref()).await?;
    status(sink.as_ref(), ready_to_send_message(&prepared));
    let reverse = target_addr.is_some();
    let conn = if let Some((addr, session_alpn)) = target_addr {
        status(sink.as_ref(), "Connecting to receiver...");
        bounded("connecting to receiver", ep.connect(addr, &session_alpn)).await?
    } else {
        engine::accept_with_tickets(ep, ALPN, sink.as_ref()).await?
    };
    let mut session = Session::new(conn, sink.clone());
    let conn = &session.conn;
    status(sink.as_ref(), "Receiver connected.");
    let (mut send_stream, mut recv_stream) = if reverse {
        bounded("opening transfer stream", conn.open_bi()).await?
    } else {
        bounded("waiting for transfer stream", conn.accept_bi()).await?
    };
    let handshake = async {
        if reverse {
            crypto::handshake_initiator(&mut recv_stream, &mut send_stream).await
        } else {
            crypto::handshake_responder(&mut recv_stream, &mut send_stream).await
        }
    };
    let (mut transport, code) = bounded("establishing encryption", handshake).await?;
    status(
        sink.as_ref(),
        format!("Encryption established. Verification code: {code}"),
    );
    emit(sink.as_ref(), TransferEvent::HandshakeCode(code));
    send_file(&mut send_stream, &mut transport, &prepared, sink.as_ref()).await?;
    send_stream
        .finish()
        .context("failed to finish transfer stream")?;
    wait_for_done(&mut recv_stream, &mut transport).await?;
    session.complete();
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        future::{poll_fn, Future},
        task::Poll,
    };
    use tokio::io::{duplex, split, AsyncWriteExt};

    #[tokio::test]
    async fn done_is_not_complete_until_confirmation_stream_fin_is_consumed() {
        let (left, right) = duplex(4096);
        let (mut sender_read, mut sender_write) = split(left);
        let (mut receiver_read, mut receiver_write) = split(right);
        let (sender, receiver) = tokio::join!(
            crypto::handshake_initiator(&mut sender_read, &mut sender_write),
            crypto::handshake_responder(&mut receiver_read, &mut receiver_write),
        );
        let (mut sender_transport, _) = sender.unwrap();
        let (mut receiver_transport, _) = receiver.unwrap();
        crypto::encrypted_write(&mut receiver_write, &mut receiver_transport, b"DONE\n")
            .await
            .unwrap();
        let completion = wait_for_done(&mut sender_read, &mut sender_transport);
        tokio::pin!(completion);
        poll_fn(|cx| {
            assert!(
                completion.as_mut().poll(cx).is_pending(),
                "DONE payload alone must not complete before FIN"
            );
            Poll::Ready(())
        })
        .await;
        receiver_write.shutdown().await.unwrap();
        completion.await.unwrap();
    }
}
