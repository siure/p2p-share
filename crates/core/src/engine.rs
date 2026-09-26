//! A desktop or mobile session owns one endpoint across sequential transfers.
use std::future::Future;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use iroh::{
    endpoint::{presets, Connection},
    Endpoint, TransportAddr, Watcher,
};
use n0_future::StreamExt;
use tokio::sync::Mutex;

use crate::{
    events::{ConnectionPathKind, TransferEvent, TransferEventSink},
    receiver, sender, ticket,
};

pub type SharedSink = Arc<dyn TransferEventSink>;
pub(crate) const ALPN: &[u8] = b"p2p-share/2";
pub(crate) const ALPN_REVERSE: &[u8] = b"p2p-share/2-reverse";
pub(crate) const SUCCESS_CLOSE_REASON: &[u8] = b"transfer complete";
pub(crate) const NETWORK_TIMEOUT: Duration = Duration::from_secs(30);

pub struct TransferEngine {
    pub(crate) endpoint: Endpoint,
    active: Mutex<()>,
}

impl TransferEngine {
    /// Start network discovery in the background without waiting for a relay.
    pub async fn bind() -> Result<Self> {
        let endpoint = bounded(
            "starting network endpoint",
            Endpoint::builder(presets::N0).alpns(Vec::new()).bind(),
        )
        .await?;
        Ok(Self {
            endpoint,
            active: Mutex::new(()),
        })
    }

    pub async fn send_paths(
        &self,
        paths: &[PathBuf],
        to: Option<&str>,
        sink: Option<SharedSink>,
    ) -> Result<()> {
        let _active = self
            .active
            .try_lock()
            .context("another transfer is already active")?;
        sender::send_on_endpoint(&self.endpoint, paths, to, sink).await
    }

    pub async fn receive(
        &self,
        target: Option<&str>,
        output_dir: &Path,
        sink: Option<SharedSink>,
    ) -> Result<()> {
        let _active = self
            .active
            .try_lock()
            .context("another transfer is already active")?;
        receiver::receive_on_endpoint(&self.endpoint, target, output_dir, sink).await
    }

    /// Graceful shutdown belongs to the application lifecycle, not each transfer.
    pub async fn close(&self) {
        self.endpoint.close().await;
    }
}

pub(crate) fn emit(sink: Option<&SharedSink>, event: TransferEvent) {
    if let Some(sink) = sink {
        sink.on_event(event);
    }
}
pub(crate) fn status(sink: Option<&SharedSink>, message: impl Into<String>) {
    let message = message.into();
    if sink.is_none() {
        eprintln!("{message}");
    }
    emit(sink, TransferEvent::Status(message));
}

pub(crate) async fn bounded<T, E>(
    phase: &str,
    future: impl Future<Output = std::result::Result<T, E>>,
) -> Result<T>
where
    E: Into<anyhow::Error>,
{
    tokio::time::timeout(NETWORK_TIMEOUT, future)
        .await
        .with_context(|| format!("Timed out while {phase}. Check the connection and try again."))?
        .map_err(Into::into)
        .with_context(|| format!("Failed while {phase}"))
}

/// Publish the first useful direct address immediately, then refresh the ticket
/// as relay/public addresses appear. Waiting for a person to connect has no timeout.
pub(crate) async fn accept_with_tickets(
    ep: &Endpoint,
    alpn: &[u8],
    sink: Option<&SharedSink>,
) -> Result<Connection> {
    let nonce = iroh::SecretKey::generate().public().to_string();
    let session_alpn = format!("{}/{nonce}", std::str::from_utf8(alpn)?);
    ep.set_alpns(vec![session_alpn.as_bytes().to_vec()]);
    let _registration = ListenerRegistration(ep.clone());
    let mut updates = ep.watch_addr().stream();
    let readiness = tokio::time::sleep(Duration::from_secs(10));
    let relay_notice = tokio::time::sleep(Duration::from_secs(5));
    tokio::pin!(readiness, relay_notice);
    let mut previous = None;
    let mut relay_reported = false;
    loop {
        tokio::select! {
            address = updates.next() => {
                let Some(address) = address else { bail!("network endpoint closed"); };
                if !ticket::has_usable_address(&address) { continue; }
                let current = ticket::serialize_session(&address, Some(&session_alpn))?;
                if previous.as_ref() != Some(&current) {
                    let has_relay = address.relay_urls().next().is_some();
                    status(sink, if has_relay { "Ready for direct or relay connections." } else { "Ready for direct connections. Connecting to relay in the background..." });
                    relay_reported |= has_relay;
                    emit(sink, TransferEvent::Ticket(current.clone()));
                    emit(sink, TransferEvent::QrPayload(current.clone()));
                    if sink.is_none() {
                        eprintln!("\nConnection ticket: {current}");
                        if let Ok(qr) = qr2term::generate_qr_string(&current) { eprintln!("{qr}"); }
                    }
                    previous = Some(current);
                }
            }
            _ = &mut readiness, if previous.is_none() => bail!("No usable network address after 10 seconds. Check your network and try again."),
            _ = &mut relay_notice, if !relay_reported => {
                status(sink, "Relay is not ready yet. Direct connections remain available; the ticket will update when the relay connects.");
                relay_reported = true;
            }
            incoming = ep.accept() => {
                let incoming = incoming.context("network endpoint closed")?;
                let conn = match bounded("accepting peer connection", async { incoming.await }).await {
                    Ok(conn) => conn,
                    Err(_) => continue,
                };
                if conn.alpn() != session_alpn.as_bytes() {
                    conn.close(1u8.into(), b"wrong transfer direction");
                    continue;
                }
                return Ok(conn);
            }
        }
    }
}

/// Closes a canceled/failed transfer and aborts its observer without closing the
/// shared endpoint. No detached task retains an old session after cancellation.
pub(crate) struct Session {
    pub(crate) conn: Connection,
    watcher: tokio::task::JoinHandle<()>,
    completed: bool,
}
impl Session {
    pub(crate) fn new(conn: Connection, sink: Option<SharedSink>) -> Self {
        let observed = conn.clone();
        let watcher = tokio::spawn(async move {
            let mut paths = observed.paths_stream();
            let mut previous: Option<(TransportAddr, std::time::Instant)> = None;
            while let Some(snapshot) = paths.next().await {
                if let Some(path) = snapshot.iter().find(|p| p.is_selected()) {
                    let now = std::time::Instant::now();
                    if previous.as_ref().is_some_and(|(addr, at)| {
                        addr == path.remote_addr()
                            && now.duration_since(*at) < Duration::from_secs(1)
                    }) {
                        continue;
                    }
                    previous = Some((path.remote_addr().clone(), now));
                    let kind = match path.remote_addr() {
                        TransportAddr::Ip(addr) => ConnectionPathKind::Direct(addr.to_string()),
                        TransportAddr::Relay(url) => ConnectionPathKind::Relay(url.to_string()),
                        _ => ConnectionPathKind::None,
                    };
                    emit(
                        sink.as_ref(),
                        TransferEvent::ConnectionPath {
                            kind,
                            latency_ms: Some(path.rtt().as_secs_f64() * 1000.0),
                        },
                    );
                }
            }
        });
        Self {
            conn,
            watcher,
            completed: false,
        }
    }

    pub(crate) fn complete(&mut self) {
        self.completed = true;
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.watcher.abort();
        if self.completed {
            self.conn.close(0u8.into(), SUCCESS_CLOSE_REASON);
        } else {
            self.conn.close(1u8.into(), b"transfer canceled or failed");
        }
    }
}

/// Partial files are removed on cancellation as well as ordinary errors.
pub(crate) struct PartialFile(pub(crate) Option<PathBuf>);
impl PartialFile {
    pub(crate) async fn create(path: PathBuf) -> Result<(tokio::fs::File, Self)> {
        // If the awaiting task is canceled while open() is in flight, the
        // blocking job's returned guard still removes the newly created file.
        let (file, cleanup) = tokio::task::spawn_blocking(move || -> Result<_> {
            let file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            Ok((file, Self(Some(path))))
        })
        .await??;
        Ok((tokio::fs::File::from_std(file), cleanup))
    }
}

impl Drop for PartialFile {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

struct ListenerRegistration(Endpoint);
impl Drop for ListenerRegistration {
    fn drop(&mut self) {
        self.0.set_alpns(Vec::new());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn local_engine() -> Arc<TransferEngine> {
        let endpoint = Endpoint::builder(presets::Minimal)
            .relay_mode(iroh::RelayMode::Disabled)
            .bind()
            .await
            .unwrap();
        Arc::new(TransferEngine {
            endpoint,
            active: Mutex::new(()),
        })
    }

    fn tickets() -> (SharedSink, tokio::sync::mpsc::UnboundedReceiver<String>) {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let sink: SharedSink = Arc::new(move |event| {
            if let TransferEvent::Ticket(ticket) = event {
                let _ = tx.send(ticket);
            }
        });
        (sink, rx)
    }

    #[tokio::test]
    async fn direct_ticket_is_prompt_and_canceled_session_cannot_consume_next_transfer() {
        let source = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let path = source.path().join("tiny.txt");
        std::fs::write(&path, b"small transfer without a relay").unwrap();
        let sender = local_engine().await;
        let receiver = local_engine().await;
        let (sink, mut ticket_rx) = tickets();
        let task = {
            let sender = sender.clone();
            let path = path.clone();
            let sink = sink.clone();
            tokio::spawn(async move { sender.send_paths(&[path], None, Some(sink)).await })
        };
        let first = tokio::time::timeout(Duration::from_secs(2), ticket_rx.recv())
            .await
            .unwrap()
            .unwrap();
        task.abort();
        let _ = task.await;
        while ticket_rx.try_recv().is_ok() {}
        let next = {
            let sender = sender.clone();
            let path = path.clone();
            let sink = sink.clone();
            tokio::spawn(async move { sender.send_paths(&[path], None, Some(sink)).await })
        };
        let second = tokio::time::timeout(Duration::from_secs(2), ticket_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            ticket::deserialize(&first).unwrap().id,
            ticket::deserialize(&second).unwrap().id
        );
        assert_ne!(
            first, second,
            "each listening operation has its own capability"
        );
        let stale = tokio::time::timeout(
            Duration::from_secs(4),
            receiver.receive(Some(&first), output.path(), Some(sink.clone())),
        )
        .await
        .unwrap();
        assert!(
            stale.is_err(),
            "a canceled ticket must not authorize another transfer"
        );
        tokio::time::timeout(
            Duration::from_secs(5),
            receiver.receive(Some(&second), output.path(), Some(sink)),
        )
        .await
        .unwrap()
        .unwrap();
        next.await.unwrap().unwrap();
        assert_eq!(
            std::fs::read(output.path().join("tiny.txt")).unwrap(),
            b"small transfer without a relay"
        );
        tokio::join!(sender.close(), receiver.close());
    }
    #[tokio::test]
    async fn cancel_mid_receive_removes_partial_and_engine_can_receive_again() {
        let output = tempfile::tempdir().unwrap();
        let source = tempfile::tempdir().unwrap();
        let path = source.path().join("complete.txt");
        std::fs::write(&path, b"next transfer").unwrap();
        let receiver = local_engine().await;
        let sender = local_engine().await;
        let (sink, mut ticket_rx) = tickets();
        let listening = {
            let receiver = receiver.clone();
            let dir = output.path().to_path_buf();
            let sink = sink.clone();
            tokio::spawn(async move { receiver.receive(None, &dir, Some(sink)).await })
        };
        let ticket = tokio::time::timeout(Duration::from_secs(2), ticket_rx.recv())
            .await
            .unwrap()
            .unwrap();
        let (addr, alpn) = ticket::connection_target(&ticket, ALPN_REVERSE).unwrap();
        let conn = sender.endpoint.connect(addr, &alpn).await.unwrap();
        let (mut send, mut recv) = conn.open_bi().await.unwrap();
        let (mut transport, _) = crate::crypto::handshake_initiator(&mut recv, &mut send)
            .await
            .unwrap();
        let header = crate::protocol::FileHeader {
            name: "partial.txt".into(),
            size: 100_000,
            blake3: "0".repeat(64),
            content_kind: None,
            item_count: None,
            logical_name: None,
        };
        crate::crypto::encrypted_write(&mut send, &mut transport, &header.to_wire().unwrap())
            .await
            .unwrap();
        crate::crypto::encrypted_write(&mut send, &mut transport, b"partial")
            .await
            .unwrap();
        let partial = output.path().join("partial.txt.part");
        tokio::time::timeout(Duration::from_secs(2), async {
            while !partial.exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        listening.abort();
        let _ = listening.await;
        assert!(
            !partial.exists(),
            "canceling receive must remove the partial output"
        );
        assert!(!output.path().join("partial.txt").exists());
        let close = tokio::time::timeout(Duration::from_secs(2), conn.closed())
            .await
            .unwrap();
        assert!(
            matches!(close, iroh::endpoint::ConnectionError::ApplicationClosed(reason)
            if reason.error_code == iroh::endpoint::VarInt::from_u32(1)),
            "a canceled receiver must not signal successful completion"
        );
        while ticket_rx.try_recv().is_ok() {}
        let next = {
            let receiver = receiver.clone();
            let dir = output.path().to_path_buf();
            let sink = sink.clone();
            tokio::spawn(async move { receiver.receive(None, &dir, Some(sink)).await })
        };
        let ticket = tokio::time::timeout(Duration::from_secs(2), ticket_rx.recv())
            .await
            .unwrap()
            .unwrap();
        tokio::time::timeout(
            Duration::from_secs(5),
            sender.send_paths(&[path], Some(&ticket), Some(sink)),
        )
        .await
        .unwrap()
        .unwrap();
        next.await.unwrap().unwrap();
        assert_eq!(
            std::fs::read(output.path().join("complete.txt")).unwrap(),
            b"next transfer"
        );
        tokio::join!(sender.close(), receiver.close());
    }
    /// Optional validation of public-relay interoperability after transport upgrades.
    #[tokio::test]
    #[ignore = "requires access to the public iroh relay service"]
    async fn public_relay_transfers_without_ip_transports() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        tokio::time::timeout(Duration::from_secs(60), async {
            let sender = Arc::new(TransferEngine {
                endpoint: Endpoint::builder(presets::N0)
                    .clear_ip_transports()
                    .bind()
                    .await
                    .unwrap(),
                active: Mutex::new(()),
            });
            let receiver = Arc::new(TransferEngine {
                endpoint: Endpoint::builder(presets::N0)
                    .clear_ip_transports()
                    .bind()
                    .await
                    .unwrap(),
                active: Mutex::new(()),
            });
            let root = tempfile::tempdir().unwrap();
            let path = root.path().join("relay.txt");
            let bytes = vec![42; 1024];
            std::fs::write(&path, &bytes).unwrap();
            let relayed = Arc::new(AtomicUsize::new(0));
            let other_path = Arc::new(AtomicBool::new(false));
            for reverse in [false, true] {
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
                let relayed = relayed.clone();
                let other_path = other_path.clone();
                let sink: SharedSink = Arc::new(move |event| match event {
                    TransferEvent::Ticket(ticket) => {
                        let _ = tx.send(ticket);
                    }
                    TransferEvent::ConnectionPath {
                        kind: ConnectionPathKind::Relay(_),
                        ..
                    } => {
                        relayed.fetch_add(1, Ordering::Relaxed);
                    }
                    TransferEvent::ConnectionPath { .. } => {
                        other_path.store(true, Ordering::Relaxed);
                    }
                    _ => {}
                });
                let output = root.path().join(if reverse { "reverse" } else { "normal" });
                let task = {
                    let sender = sender.clone();
                    let receiver = receiver.clone();
                    let path = path.clone();
                    let output = output.clone();
                    let sink = sink.clone();
                    tokio::spawn(async move {
                        if reverse {
                            receiver.receive(None, &output, Some(sink)).await
                        } else {
                            sender.send_paths(&[path], None, Some(sink)).await
                        }
                    })
                };
                let ticket = rx.recv().await.unwrap();
                assert!(ticket::deserialize(&ticket)
                    .unwrap()
                    .ip_addrs()
                    .next()
                    .is_none());
                if reverse {
                    sender
                        .send_paths(std::slice::from_ref(&path), Some(&ticket), Some(sink))
                        .await
                        .unwrap();
                } else {
                    receiver
                        .receive(Some(&ticket), &output, Some(sink))
                        .await
                        .unwrap();
                }
                task.await.unwrap().unwrap();
                assert_eq!(std::fs::read(output.join("relay.txt")).unwrap(), bytes);
            }
            assert!(relayed.load(Ordering::Relaxed) >= 2);
            assert!(
                !other_path.load(Ordering::Relaxed),
                "IP transports must remain disabled for this check"
            );
            tokio::join!(sender.close(), receiver.close());
        })
        .await
        .expect("public-relay transfer check exceeded 60 seconds");
    }
}
