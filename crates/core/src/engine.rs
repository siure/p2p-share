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
    events::{ConnectionPathKind, TransferEvent, TransferEventSink, TransferOffer},
    nearby, receiver, sender, ticket,
};

pub type SharedSink = Arc<dyn TransferEventSink>;
pub(crate) const ALPN: &[u8] = b"p2p-share/2";
pub(crate) const ALPN_REVERSE: &[u8] = b"p2p-share/2-reverse";
/// Like reverse mode, but the receiver is advertised to nearby devices and
/// must accept each offer before file data flows.
pub(crate) const ALPN_NEARBY: &[u8] = b"p2p-share/2-nearby";
/// How long a nearby receiver may take to accept or decline an offer.
pub(crate) const OFFER_TIMEOUT: Duration = Duration::from_secs(120);
pub(crate) const SUCCESS_CLOSE_REASON: &[u8] = b"transfer complete";
pub(crate) const NETWORK_TIMEOUT: Duration = Duration::from_secs(30);

type OfferSlot = std::sync::Mutex<Option<tokio::sync::oneshot::Sender<bool>>>;

pub struct TransferEngine {
    pub(crate) endpoint: Endpoint,
    active: Mutex<()>,
    device_name: std::sync::RwLock<String>,
    offer: OfferSlot,
}

impl TransferEngine {
    /// Start network discovery in the background without waiting for a relay.
    pub async fn bind() -> Result<Self> {
        let endpoint = bounded(
            "starting network endpoint",
            Endpoint::builder(presets::N0).alpns(Vec::new()).bind(),
        )
        .await?;
        Ok(Self::from_endpoint(endpoint))
    }

    pub(crate) fn from_endpoint(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            active: Mutex::new(()),
            device_name: std::sync::RwLock::new(nearby::default_device_name()),
            offer: std::sync::Mutex::new(None),
        }
    }

    /// Name shown to nearby devices and in their accept prompts.
    pub fn set_device_name(&self, name: &str) {
        let name = name.trim();
        if !name.is_empty() {
            *self.device_name.write().unwrap_or_else(|e| e.into_inner()) = name.to_string();
        }
    }

    pub fn device_name(&self) -> String {
        self.device_name
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// Whether this device currently reaches the relay, i.e. has internet access.
    pub fn has_relay(&self) -> bool {
        self.endpoint.addr().relay_urls().next().is_some()
    }

    /// Answer the pending `TransferEvent::Offer`. Returns false if none is pending.
    pub fn answer_offer(&self, accept: bool) -> bool {
        let pending = self.offer.lock().unwrap_or_else(|e| e.into_inner()).take();
        pending.is_some_and(|reply| reply.send(accept).is_ok())
    }

    /// Wait for one nearby sender while advertising this device over mDNS (and
    /// Bluetooth where supported). Declined offers keep the device visible.
    pub async fn receive_nearby(&self, output_dir: &Path, sink: Option<SharedSink>) -> Result<()> {
        let _active = self
            .active
            .try_lock()
            .context("another transfer is already active")?;
        receiver::receive_nearby_on_endpoint(self, output_dir, sink).await
    }

    /// List devices that are currently receiving nearby. Dropping the browser stops it.
    pub fn browse_nearby(
        &self,
        on_event: impl Fn(nearby::NearbyEvent) + Send + Sync + 'static,
    ) -> nearby::Browser {
        nearby::Browser::start(self.endpoint.id().to_string(), on_event)
    }

    /// Send to a discovered device, first bringing up a shared network over
    /// Bluetooth when neither LAN nor internet can reach it.
    pub async fn send_nearby(
        &self,
        paths: &[PathBuf],
        peer: &nearby::NearbyPeer,
        sink: Option<SharedSink>,
    ) -> Result<()> {
        let _active = self
            .active
            .try_lock()
            .context("another transfer is already active")?;
        let (ticket, _network) = nearby::reach(peer, self.has_relay(), sink.as_ref()).await?;
        sender::send_on_endpoint(
            &self.endpoint,
            paths,
            Some(&ticket),
            &self.device_name(),
            sink,
        )
        .await
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
        sender::send_on_endpoint(&self.endpoint, paths, to, &self.device_name(), sink).await
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

/// Lines typed on the terminal. One reader thread outlives timed-out prompts,
/// so a late answer is never consumed by an abandoned read.
fn terminal_lines() -> &'static std::sync::Mutex<std::sync::mpsc::Receiver<String>> {
    static LINES: std::sync::OnceLock<std::sync::Mutex<std::sync::mpsc::Receiver<String>>> =
        std::sync::OnceLock::new();
    LINES.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::stdin().lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        std::sync::Mutex::new(rx)
    })
}

/// Emit an offer and wait for `TransferEngine::answer_offer`. Without a sink
/// (interactive CLI) the question is asked on the terminal instead.
pub(crate) async fn ask_offer(
    engine: &TransferEngine,
    sink: Option<&SharedSink>,
    offer: TransferOffer,
) -> bool {
    if sink.is_none() {
        let from = offer.sender_name.as_deref().unwrap_or("A nearby device");
        eprint!(
            "\n{from} wants to send {} ({}). Accept? [y/N] ",
            offer.file_name,
            crate::protocol::human_bytes(offer.size_bytes)
        );
        // Same deadline as the event path, so an unanswered prompt declines and
        // the receiver goes back to listening.
        let answer = tokio::task::spawn_blocking(|| {
            let lines = terminal_lines().lock().unwrap_or_else(|e| e.into_inner());
            while lines.try_recv().is_ok() {} // drop input typed before the prompt
            lines.recv_timeout(OFFER_TIMEOUT).ok()
        })
        .await
        .ok()
        .flatten();
        if answer.is_none() {
            eprintln!("No answer; declined.");
        }
        answer.is_some_and(|line| {
            line.trim().eq_ignore_ascii_case("y") || line.trim().eq_ignore_ascii_case("yes")
        })
    } else {
        let (reply, answer) = tokio::sync::oneshot::channel();
        *engine.offer.lock().unwrap_or_else(|e| e.into_inner()) = Some(reply);
        emit(sink, TransferEvent::Offer(offer));
        let accepted = tokio::time::timeout(OFFER_TIMEOUT, answer).await;
        engine
            .offer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        matches!(accepted, Ok(Ok(true)))
    }
}

/// Publish the first useful direct address immediately, then refresh the ticket
/// as relay/public addresses appear. Waiting for a person to connect has no timeout.
/// `publish` also receives every ticket, e.g. to update nearby advertisements.
pub(crate) async fn accept_with_tickets(
    ep: &Endpoint,
    alpn: &[u8],
    sink: Option<&SharedSink>,
    publish: &(dyn Fn(&str) + Sync),
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
                    publish(&current);
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
        Arc::new(TransferEngine::from_endpoint(endpoint))
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
        let (addr, alpn) = ticket::connection_target(&ticket, &[ALPN_REVERSE]).unwrap();
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
            sender_name: None,
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
    #[tokio::test]
    async fn nearby_receiver_declines_then_accepts_and_stays_visible_in_between() {
        let source = tempfile::tempdir().unwrap();
        let output = tempfile::tempdir().unwrap();
        let path = source.path().join("offer.txt");
        std::fs::write(&path, b"nearby payload").unwrap();
        let sender = local_engine().await;
        sender.set_device_name("Test laptop");
        let receiver = local_engine().await;
        let (tx, mut events) = tokio::sync::mpsc::unbounded_channel();
        let sink: SharedSink = Arc::new(move |event| {
            let _ = tx.send(event);
        });
        let listening = {
            let receiver = receiver.clone();
            let dir = output.path().to_path_buf();
            tokio::spawn(async move { receiver.receive_nearby(&dir, Some(sink)).await })
        };
        let next = |events: &mut tokio::sync::mpsc::UnboundedReceiver<TransferEvent>| {
            let mut ticket = None;
            loop {
                match events.try_recv() {
                    Ok(TransferEvent::Ticket(t)) => ticket = Some(t),
                    Ok(TransferEvent::Offer(offer)) => return (ticket, Some(offer)),
                    Ok(_) => {}
                    Err(_) => return (ticket, None),
                }
            }
        };
        let wait_ticket =
            async |events: &mut tokio::sync::mpsc::UnboundedReceiver<TransferEvent>| loop {
                if let Some(TransferEvent::Ticket(t)) = events.recv().await {
                    return t;
                }
            };
        let first = tokio::time::timeout(Duration::from_secs(5), wait_ticket(&mut events))
            .await
            .unwrap();
        assert!(ticket::connection_target(&first, &[ALPN_NEARBY]).is_ok());

        let answer = |accept: bool| {
            let receiver = receiver.clone();
            tokio::spawn(async move {
                while !receiver.answer_offer(accept) {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
        };
        let decline = answer(false);
        let declined = sender
            .send_paths(std::slice::from_ref(&path), Some(&first), None)
            .await
            .unwrap_err();
        decline.await.unwrap();
        assert!(format!("{declined:#}").contains("declined"), "{declined:#}");
        let (_, offer) = next(&mut events);
        let offer = offer.expect("receiver was asked");
        assert_eq!(offer.sender_name.as_deref(), Some("Test laptop"));
        assert_eq!(offer.size_bytes, 14);
        assert!(!output.path().join("offer.txt").exists());

        let second = tokio::time::timeout(Duration::from_secs(5), wait_ticket(&mut events))
            .await
            .unwrap();
        assert_ne!(first, second, "a new session follows a declined offer");
        let accept = answer(true);
        sender
            .send_paths(std::slice::from_ref(&path), Some(&second), None)
            .await
            .unwrap();
        accept.await.unwrap();
        listening.await.unwrap().unwrap();
        assert_eq!(
            std::fs::read(output.path().join("offer.txt")).unwrap(),
            b"nearby payload"
        );
        tokio::join!(sender.close(), receiver.close());
    }

    /// Optional validation of public-relay interoperability after transport upgrades.
    #[tokio::test]
    #[ignore = "requires access to the public iroh relay service"]
    async fn public_relay_transfers_without_ip_transports() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        tokio::time::timeout(Duration::from_secs(60), async {
            let sender = Arc::new(TransferEngine::from_endpoint(
                Endpoint::builder(presets::N0)
                    .clear_ip_transports()
                    .bind()
                    .await
                    .unwrap(),
            ));
            let receiver = Arc::new(TransferEngine::from_endpoint(
                Endpoint::builder(presets::N0)
                    .clear_ip_transports()
                    .bind()
                    .await
                    .unwrap(),
            ));
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
