use anyhow::{bail, Context, Result};
use iroh::{
    endpoint::{Connection, ConnectionError, SendStream, StoppedError, VarInt},
    Endpoint,
};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

use crate::bundle;
use crate::crypto;
use crate::events::{TransferCompleted, TransferContentKind, TransferEvent, TransferOffer};
use crate::nearby;
use crate::progress::transfer_progress_bar;
use crate::protocol::{human_bytes, FileHeader, OFFER_ACCEPT, OFFER_DECLINE};
use crate::ticket;

use crate::engine::{
    self, bounded, emit, status, PartialFile, Session, SharedSink, TransferEngine, ALPN,
    ALPN_NEARBY, ALPN_REVERSE,
};

/// The receiver turned an offer down; nearby mode keeps listening afterwards.
#[derive(Debug)]
struct OfferDeclined;
impl std::fmt::Display for OfferDeclined {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Transfer declined.")
    }
}
impl std::error::Error for OfferDeclined {}

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

fn bundle_logical_name(header: &FileHeader) -> String {
    header
        .logical_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| bundle::logical_name_from_wire_name(&header.name))
}

/// One-shot helpers; desktop/mobile callers should keep a TransferEngine.
pub async fn run(target: &str, output_dir: &Path) -> Result<()> {
    run_with_sink(target, output_dir, None).await
}
pub async fn run_with_sink(
    target: &str,
    output_dir: &Path,
    sink: Option<SharedSink>,
) -> Result<()> {
    if !ticket::is_ticket(target) {
        return run_direct_tcp(target.trim(), output_dir, sink).await;
    }
    let engine = TransferEngine::bind().await?;
    let result = engine.receive(Some(target), output_dir, sink).await;
    engine.close().await;
    result
}
pub async fn run_listen(output_dir: &Path) -> Result<()> {
    run_listen_with_sink(output_dir, None).await
}
pub async fn run_listen_with_sink(output_dir: &Path, sink: Option<SharedSink>) -> Result<()> {
    let engine = TransferEngine::bind().await?;
    let result = engine.receive(None, output_dir, sink).await;
    engine.close().await;
    result
}

pub(crate) async fn receive_on_endpoint(
    ep: &Endpoint,
    target: Option<&str>,
    output_dir: &Path,
    sink: Option<SharedSink>,
) -> Result<()> {
    let target = target.map(str::trim);
    if let Some(target) = target.filter(|t| !ticket::is_ticket(t)) {
        return run_direct_tcp(target, output_dir, sink).await;
    }
    let conn = if let Some(target) = target {
        status(sink.as_ref(), "Connecting to sender...");
        let (addr, session_alpn) = ticket::connection_target(target, &[ALPN])?;
        bounded("connecting to sender", ep.connect(addr, &session_alpn)).await?
    } else {
        engine::accept_with_tickets(ep, ALPN_REVERSE, sink.as_ref(), &|_| {}).await?
    };
    let mut session = Session::new(conn, sink.clone());
    let conn = &session.conn;
    status(sink.as_ref(), "Sender connected.");
    let (mut send_stream, mut recv_stream) = if target.is_some() {
        bounded("opening transfer stream", conn.open_bi()).await?
    } else {
        bounded("waiting for transfer stream", conn.accept_bi()).await?
    };
    let handshake = async {
        if target.is_some() {
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
    let completed = receive_file(
        &mut recv_stream,
        &mut send_stream,
        &mut transport,
        output_dir,
        sink.as_ref(),
        false,
        None,
    )
    .await?;
    confirm_done_delivery(conn, &mut send_stream).await?;
    session.complete();
    emit(sink.as_ref(), TransferEvent::Completed(completed));
    Ok(())
}

pub(crate) async fn receive_nearby_on_endpoint(
    engine: &TransferEngine,
    output_dir: &Path,
    sink: Option<SharedSink>,
) -> Result<()> {
    let ep = &engine.endpoint;
    let name = engine.device_name();
    let advertiser = nearby::Advertiser::start(&name, sink.as_ref()).await;
    status(
        sink.as_ref(),
        format!("Visible to nearby devices as \"{name}\"."),
    );
    let publish = |ticket: &str| match nearby::Beacon::new(&name, ticket) {
        Ok(beacon) => {
            advertiser.update(ticket, &beacon);
            emit(sink.as_ref(), TransferEvent::Beacon(beacon.to_json()));
        }
        Err(err) => status(sink.as_ref(), format!("Nearby beacon unavailable: {err:#}")),
    };
    loop {
        let conn = engine::accept_with_tickets(ep, ALPN_NEARBY, sink.as_ref(), &publish).await?;
        let mut session = Session::new(conn, sink.clone());
        let conn = &session.conn;
        status(sink.as_ref(), "Sender connected.");
        let (mut send_stream, mut recv_stream) =
            bounded("waiting for transfer stream", conn.accept_bi()).await?;
        let (mut transport, code) = bounded(
            "establishing encryption",
            crypto::handshake_responder(&mut recv_stream, &mut send_stream),
        )
        .await?;
        emit(sink.as_ref(), TransferEvent::HandshakeCode(code));
        let result = receive_file(
            &mut recv_stream,
            &mut send_stream,
            &mut transport,
            output_dir,
            sink.as_ref(),
            false,
            Some(engine),
        )
        .await;
        match result {
            Err(err) if err.is::<OfferDeclined>() => {
                // Deliver DECLINE before the session closes the connection.
                if send_stream.finish().is_ok() {
                    let _ = tokio::time::timeout(
                        std::time::Duration::from_secs(3),
                        send_stream.stopped(),
                    )
                    .await;
                }
                status(sink.as_ref(), "Declined. Still visible to nearby devices.");
            }
            result => {
                let completed = result?;
                confirm_done_delivery(conn, &mut send_stream).await?;
                session.complete();
                emit(sink.as_ref(), TransferEvent::Completed(completed));
                return Ok(());
            }
        }
    }
}

async fn run_direct_tcp(addr: &str, output_dir: &Path, sink: Option<SharedSink>) -> Result<()> {
    status(sink.as_ref(), format!("Connecting to {addr}..."));
    let stream = bounded("connecting to sender", tokio::net::TcpStream::connect(addr)).await?;
    stream.set_nodelay(true)?;
    let (mut reader, mut writer) = stream.into_split();
    let (mut transport, code) = bounded(
        "establishing encryption",
        crypto::handshake_initiator(&mut reader, &mut writer),
    )
    .await?;
    emit(sink.as_ref(), TransferEvent::HandshakeCode(code));
    let completed = receive_file(
        &mut reader,
        &mut writer,
        &mut transport,
        output_dir,
        sink.as_ref(),
        true,
        None,
    )
    .await?;
    emit(sink.as_ref(), TransferEvent::Completed(completed));
    Ok(())
}

/// Shared file-reception logic used by all receive paths (iroh, TCP, listen).
/// With `approval`, the person is asked before any data is accepted.
async fn receive_file<R, W>(
    reader: &mut R,
    writer: &mut W,
    transport: &mut snow::TransportState,
    output_dir: &Path,
    sink: Option<&SharedSink>,
    legacy_tcp_ack: bool,
    approval: Option<&TransferEngine>,
) -> Result<TransferCompleted>
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    let header_bytes = bounded(
        "waiting for file metadata",
        crypto::encrypted_read(reader, transport),
    )
    .await?;
    let header_str = String::from_utf8(header_bytes).context("invalid UTF-8 in file header")?;

    if header_str.is_empty() {
        bail!("Connection closed before receiving file header");
    }

    let header = FileHeader::from_wire(&header_str)?;
    let content_kind = header.content_kind.unwrap_or(TransferContentKind::File);
    let item_count = header.item_count.unwrap_or(1).max(1);
    let completed_name = if content_kind == TransferContentKind::Bundle {
        bundle_logical_name(&header)
    } else {
        header.name.clone()
    };

    eprintln!();
    let incoming_status = if content_kind == TransferContentKind::Bundle {
        format!(
            "Incoming files: {} ({} files, {})",
            completed_name,
            item_count,
            human_bytes(header.size)
        )
    } else {
        format!(
            "Incoming file: {} ({})",
            header.name,
            human_bytes(header.size)
        )
    };
    status(sink, incoming_status);

    if let Some(engine) = approval {
        let offer = TransferOffer {
            sender_name: header.sender_name.clone(),
            file_name: completed_name.clone(),
            size_bytes: header.size,
            content_kind,
            item_count,
        };
        let accepted = engine::ask_offer(engine, sink, offer).await;
        let verdict = if accepted {
            OFFER_ACCEPT
        } else {
            OFFER_DECLINE
        };
        bounded(
            "answering the transfer offer",
            crypto::encrypted_write(writer, transport, verdict),
        )
        .await?;
        if !accepted {
            return Err(OfferDeclined.into());
        }
    }

    if legacy_tcp_ack {
        bounded(
            "accepting legacy TCP transfer",
            crypto::encrypted_write(writer, transport, b"OK\n"),
        )
        .await?;
    }

    tokio::fs::create_dir_all(output_dir).await?;
    let temp_dest = unique_path(output_dir, &format!("{}.part", header.name));

    let final_dest = if content_kind == TransferContentKind::Bundle {
        unique_path(output_dir, &completed_name)
    } else {
        unique_path(output_dir, &header.name)
    };

    if content_kind == TransferContentKind::Bundle {
        status(
            sink,
            format!("Saving extracted files to: {}", final_dest.display()),
        );
    } else {
        status(sink, format!("Saving to: {}", final_dest.display()));
    }
    eprintln!();

    let (mut file, mut partial) = PartialFile::create(temp_dest.clone()).await?;
    let pb = if sink.is_none() {
        Some(transfer_progress_bar(header.size))
    } else {
        None
    };
    let mut received: u64 = 0;
    let mut ciphertext = Vec::with_capacity(crate::protocol::CHUNK_SIZE + 16);
    let mut plaintext = Vec::with_capacity(crate::protocol::CHUNK_SIZE);
    let mut progress = crate::progress::ProgressThrottle::default();
    let mut hasher = blake3::Hasher::new();
    let receive_result: Result<(PathBuf, u64)> = async {
        while received < header.size {
            bounded(
                "receiving file data",
                crypto::encrypted_read_with_buffers(
                    reader,
                    transport,
                    &mut ciphertext,
                    &mut plaintext,
                ),
            )
            .await?;
            if plaintext.is_empty() {
                break;
            }

            file.write_all(&plaintext).await?;
            hasher.update(&plaintext);
            received += plaintext.len() as u64;
            if let Some(pb) = &pb {
                pb.set_position(received);
            }
            if progress.should_emit(received, header.size) {
                emit(
                    sink,
                    TransferEvent::Progress {
                        done: received,
                        total: header.size,
                    },
                );
            }
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

        let final_count = if content_kind == TransferContentKind::Bundle {
            let archive = partial.0.take().expect("received archive has an owner");
            bundle::extract_bundle_owned(archive, &final_dest).await?
        } else {
            tokio::fs::rename(&temp_dest, &final_dest).await?;
            1
        };
        partial.0 = None;
        bounded(
            "sending receiver confirmation",
            crypto::encrypted_write(writer, transport, b"DONE\n"),
        )
        .await?;
        Ok((final_dest.clone(), final_count))
    }
    .await;

    let (saved_path, completed_count) = match receive_result {
        Ok(result) => result,
        Err(err) => {
            let _ = tokio::fs::remove_file(&temp_dest).await;
            return Err(err);
        }
    };

    eprintln!();
    let success_status = if content_kind == TransferContentKind::Bundle {
        format!(
            "Files received successfully: {} ({} files, {})",
            saved_path.display(),
            completed_count,
            human_bytes(header.size)
        )
    } else {
        format!(
            "File received successfully: {} ({})",
            saved_path.display(),
            human_bytes(header.size)
        )
    };
    status(sink, success_status);
    status(sink, "Checksum verified (blake3).");
    Ok(TransferCompleted {
        file_name: completed_name,
        size_bytes: header.size,
        saved_path: Some(saved_path),
        content_kind,
        item_count: completed_count,
    })
}

fn successful_peer_close(error: &ConnectionError) -> bool {
    matches!(error, ConnectionError::ApplicationClosed(close)
        if close.error_code == VarInt::from_u32(0)
            && close.reason.as_ref() == engine::SUCCESS_CLOSE_REASON)
}

fn check_done_delivery(result: std::result::Result<Option<VarInt>, StoppedError>) -> Result<()> {
    match result {
        Ok(None) => Ok(()),
        // The sender closes only after reading DONE. Its application close can
        // reach us before QUIC acknowledges our final bytes, which is success too.
        Err(StoppedError::ConnectionLost(error)) if successful_peer_close(&error) => Ok(()),
        Ok(Some(code)) => bail!("sender stopped receiver confirmation (code {code})"),
        Err(error) => Err(error).context("failed to deliver receiver confirmation"),
    }
}

async fn confirm_done_delivery(conn: &Connection, stream: &mut SendStream) -> Result<()> {
    if let Err(error) = stream.finish() {
        if conn
            .close_reason()
            .as_ref()
            .is_some_and(successful_peer_close)
        {
            return Ok(());
        }
        return Err(error).context("failed to finish receiver confirmation");
    }
    bounded("delivering receiver confirmation", async {
        check_done_delivery(stream.stopped().await)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::endpoint::ApplicationClose;

    fn peer_close(code: u32, reason: &'static [u8]) -> StoppedError {
        StoppedError::ConnectionLost(ConnectionError::ApplicationClosed(ApplicationClose {
            error_code: VarInt::from_u32(code),
            reason: reason.into(),
        }))
    }

    #[test]
    fn final_confirmation_accepts_ack_or_explicit_success_close_only() {
        assert!(check_done_delivery(Ok(None)).is_ok());
        assert!(check_done_delivery(Err(peer_close(0, engine::SUCCESS_CLOSE_REASON))).is_ok());
        assert!(check_done_delivery(Err(peer_close(1, b"transfer canceled or failed"))).is_err());
        assert!(check_done_delivery(Err(peer_close(0, b"unrelated close"))).is_err());
        assert!(
            check_done_delivery(Err(StoppedError::ConnectionLost(ConnectionError::TimedOut)))
                .is_err()
        );
        assert!(check_done_delivery(Ok(Some(VarInt::from_u32(0)))).is_err());
    }
}
