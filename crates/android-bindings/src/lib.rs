use std::collections::VecDeque;
#[cfg(target_os = "android")]
use std::ffi::c_void;
use std::ffi::{c_char, CStr, CString};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use p2p_share_core::events::{
    ConnectionPathKind, TransferContentKind, TransferEvent, TransferEventSink,
};
use p2p_share_core::nearby::{Browser, NearbyEvent};
use p2p_share_core::{SharedSink, TransferEngine};
use serde::{Deserialize, Serialize};
use tokio::runtime::{Builder, Runtime};
use tokio::sync::OnceCell;
use tokio::task::JoinHandle;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransferEventRecord {
    pub kind: String,
    pub message: Option<String>,
    pub value: Option<String>,
    pub done: Option<u64>,
    pub total: Option<u64>,
    pub file_name: Option<String>,
    pub size_bytes: Option<u64>,
    pub saved_path: Option<String>,
    pub latency_ms: Option<f64>,
    pub content_kind: Option<String>,
    pub item_count: Option<u64>,
}

impl TransferEventRecord {
    fn status(message: impl Into<String>) -> Self {
        Self {
            kind: "status".to_string(),
            message: Some(message.into()),
            value: None,
            done: None,
            total: None,
            file_name: None,
            size_bytes: None,
            saved_path: None,
            latency_ms: None,
            content_kind: None,
            item_count: None,
        }
    }

    fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: "error".to_string(),
            message: Some(message.into()),
            value: Some(code.into()),
            done: None,
            total: None,
            file_name: None,
            size_bytes: None,
            saved_path: None,
            latency_ms: None,
            content_kind: None,
            item_count: None,
        }
    }
}

const MAX_QUEUED_EVENTS: usize = 128;

/// Peers found over mDNS. Android merges them with its own Bluetooth sightings.
#[derive(Debug, Clone, Serialize)]
struct NearbyRecord {
    kind: &'static str,
    peer_id: String,
    name: Option<String>,
    ticket: Option<String>,
}

fn map_nearby(event: NearbyEvent) -> NearbyRecord {
    match event {
        NearbyEvent::Found(peer) => NearbyRecord {
            kind: "nearby_found",
            peer_id: peer.id,
            name: Some(peer.name),
            ticket: Some(peer.ticket),
        },
        NearbyEvent::Lost(id) => NearbyRecord {
            kind: "nearby_lost",
            peer_id: id,
            name: None,
            ticket: None,
        },
    }
}

#[derive(Default)]
struct BrowseState {
    generation: u64,
    browser: Option<Browser>,
}

#[derive(Default)]
struct EventQueue {
    generation: u64,
    events: VecDeque<TransferEventRecord>,
}

struct QueueSink {
    queue: Arc<Mutex<EventQueue>>,
    generation: u64,
}

impl TransferEventSink for QueueSink {
    fn on_event(&self, event: TransferEvent) {
        push_event(&self.queue, self.generation, map_event(event));
    }
}

type EngineCell = Arc<OnceCell<TransferEngine>>;

pub struct TransferController {
    runtime: Runtime,
    /// Replaced by `rebind` so new sockets follow the process network binding.
    engine: Mutex<EngineCell>,
    device_name: Arc<Mutex<Option<String>>>,
    queue: Arc<Mutex<EventQueue>>,
    task: Mutex<Option<JoinHandle<()>>>,
    browse: Arc<Mutex<BrowseState>>,
    nearby: Arc<Mutex<VecDeque<NearbyRecord>>>,
}

impl TransferController {
    pub fn new() -> Self {
        let runtime = Builder::new_multi_thread()
            .enable_all()
            .thread_name("p2p-share-runtime")
            .build()
            .expect("failed to build tokio runtime");
        let engine = Self::warm_engine(&runtime);
        Self {
            runtime,
            engine: Mutex::new(engine),
            device_name: Arc::new(Mutex::new(None)),
            queue: Arc::new(Mutex::new(EventQueue::default())),
            task: Mutex::new(None),
            browse: Arc::new(Mutex::new(BrowseState::default())),
            nearby: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    fn warm_engine(runtime: &Runtime) -> EngineCell {
        let engine = Arc::new(OnceCell::new());
        let warm_engine = engine.clone();
        runtime.spawn(async move {
            let _ = warm_engine.get_or_try_init(TransferEngine::bind).await;
        });
        engine
    }

    fn engine(&self) -> EngineCell {
        self.engine.lock().expect("engine lock").clone()
    }

    pub fn set_device_name(&self, name: impl Into<String>) {
        *self.device_name.lock().expect("name lock") = Some(name.into());
    }

    /// Whether the engine currently reaches its relay (has internet).
    pub fn has_relay(&self) -> bool {
        self.engine().get().is_some_and(TransferEngine::has_relay)
    }

    /// Answer the pending offer of a nearby receive.
    pub fn answer_offer(&self, accept: bool) -> bool {
        self.engine()
            .get()
            .is_some_and(|engine| engine.answer_offer(accept))
    }

    /// Replace the endpoint after the process switched networks (joined hotspot).
    /// Cancels the current transfer and stops browsing.
    pub fn rebind(&self) {
        self.cancel();
        self.stop_browse();
        let old = std::mem::replace(
            &mut *self.engine.lock().expect("engine lock"),
            Self::warm_engine(&self.runtime),
        );
        let previous = self.task.lock().expect("task lock").take();
        self.runtime.spawn(async move {
            if let Some(previous) = previous {
                let _ = previous.await;
            }
            if let Some(engine) = old.get() {
                engine.close().await;
            }
        });
    }

    pub fn start_browse(&self) {
        let generation = {
            let mut state = self.browse.lock().expect("browse lock");
            state.generation += 1;
            state.browser = None;
            state.generation
        };
        let (engine, browse, nearby) = (self.engine(), self.browse.clone(), self.nearby.clone());
        self.runtime.spawn(async move {
            let Ok(engine) = engine.get_or_try_init(TransferEngine::bind).await else {
                return;
            };
            let browser = engine.browse_nearby(move |event| {
                if let Ok(mut queue) = nearby.lock() {
                    if queue.len() >= MAX_QUEUED_EVENTS {
                        queue.pop_front();
                    }
                    queue.push_back(map_nearby(event));
                }
            });
            let mut state = browse.lock().expect("browse lock");
            // A stop or restart while the engine was starting wins.
            if state.generation == generation {
                state.browser = Some(browser);
            }
        });
    }

    pub fn stop_browse(&self) {
        let mut state = self.browse.lock().expect("browse lock");
        state.generation += 1;
        state.browser = None;
    }

    pub fn poll_nearby_json(&self) -> Option<String> {
        let record = self.nearby.lock().ok()?.pop_front()?;
        serde_json::to_string(&record).ok()
    }

    /// Receive while advertised over mDNS; the app adds Bluetooth from `beacon` events.
    pub fn start_receive_nearby(&self, output_dir: impl Into<String>) {
        let output = PathBuf::from(output_dir.into());
        self.start_task(move |engine, sink| async move {
            engine
                .get_or_try_init(TransferEngine::bind)
                .await?
                .receive_nearby(&output, Some(sink))
                .await
        });
    }

    pub fn start_send_wait(&self, file_paths: Vec<String>) {
        let paths: Vec<PathBuf> = file_paths.into_iter().map(PathBuf::from).collect();
        self.start_task(move |engine, sink| async move {
            engine
                .get_or_try_init(TransferEngine::bind)
                .await?
                .send_paths(&paths, None, Some(sink))
                .await
        });
    }

    pub fn start_send_to_ticket(&self, file_paths: Vec<String>, ticket: impl Into<String>) {
        let paths: Vec<PathBuf> = file_paths.into_iter().map(PathBuf::from).collect();
        let ticket = ticket.into();
        self.start_task(move |engine, sink| async move {
            engine
                .get_or_try_init(TransferEngine::bind)
                .await?
                .send_paths(&paths, Some(&ticket), Some(sink))
                .await
        });
    }

    pub fn start_receive_target(&self, target: impl Into<String>, output_dir: impl Into<String>) {
        let target = target.into();
        let output = PathBuf::from(output_dir.into());
        self.start_task(move |engine, sink| async move {
            engine
                .get_or_try_init(TransferEngine::bind)
                .await?
                .receive(Some(&target), &output, Some(sink))
                .await
        });
    }

    pub fn start_receive_listen(&self, output_dir: impl Into<String>) {
        let output = PathBuf::from(output_dir.into());
        self.start_task(move |engine, sink| async move {
            engine
                .get_or_try_init(TransferEngine::bind)
                .await?
                .receive(None, &output, Some(sink))
                .await
        });
    }

    pub fn poll_event(&self) -> Option<TransferEventRecord> {
        self.queue.lock().ok()?.events.pop_front()
    }

    pub fn poll_event_json(&self) -> Option<String> {
        self.poll_event()
            .and_then(|evt| serde_json::to_string(&evt).ok())
    }

    pub fn cancel(&self) {
        if let Ok(task) = self.task.lock() {
            if let Some(handle) = task.as_ref() {
                handle.abort();
            }
        }
        if let Ok(mut queue) = self.queue.lock() {
            queue.generation += 1;
            queue.events.clear();
        }
    }

    fn start_task<F, Fut>(&self, action: F)
    where
        F: FnOnce(EngineCell, SharedSink) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        self.cancel();
        let previous = self.task.lock().expect("task lock").take();
        let queue = self.queue.clone();
        let generation = queue.lock().expect("queue lock").generation;
        let sink = Arc::new(QueueSink {
            queue: queue.clone(),
            generation,
        });
        let engine = self.engine();
        let name = self.device_name.lock().expect("name lock").clone();
        push_event(
            &queue,
            generation,
            TransferEventRecord::status("Transfer started."),
        );
        let task = self.runtime.spawn(async move {
            // Ensure cancellation released the engine's active-transfer lock.
            if let Some(previous) = previous {
                let _ = previous.await;
            }
            if let (Some(name), Ok(engine)) =
                (name, engine.get_or_try_init(TransferEngine::bind).await)
            {
                engine.set_device_name(&name);
            }
            if let Err(err) = action(engine, sink).await {
                push_event(
                    &queue,
                    generation,
                    TransferEventRecord::error("transfer_error", format!("{:#}", err)),
                );
            }
        });
        *self.task.lock().expect("task lock") = Some(task);
    }

    fn shutdown(self) {
        self.cancel();
        self.stop_browse();
        let engine = self.engine();
        self.runtime.block_on(async {
            if let Some(engine) = engine.get() {
                engine.close().await;
            }
        });
        // Dropping the runtime joins its workers on the destruction thread.
    }
}

impl Default for TransferController {
    fn default() -> Self {
        Self::new()
    }
}

pub fn bindings_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn push_event(queue: &Arc<Mutex<EventQueue>>, generation: u64, event: TransferEventRecord) {
    if let Ok(mut q) = queue.lock() {
        if generation != q.generation {
            return;
        }
        if event.kind == "progress" {
            q.events.retain(|old| old.kind != "progress");
        }
        if q.events.len() >= MAX_QUEUED_EVENTS {
            // Keep terminal results even if status updates outpace the UI.
            let oldest = q
                .events
                .iter()
                .position(|old| old.kind != "completed" && old.kind != "error")
                .unwrap_or(0);
            q.events.remove(oldest);
        }
        q.events.push_back(event);
    }
}

fn map_event(event: TransferEvent) -> TransferEventRecord {
    match event {
        TransferEvent::Status(message) => TransferEventRecord::status(message),
        TransferEvent::Ticket(ticket) => TransferEventRecord {
            kind: "ticket".to_string(),
            message: None,
            value: Some(ticket),
            done: None,
            total: None,
            file_name: None,
            size_bytes: None,
            saved_path: None,
            latency_ms: None,
            content_kind: None,
            item_count: None,
        },
        TransferEvent::QrPayload(payload) => TransferEventRecord {
            kind: "qr_payload".to_string(),
            message: None,
            value: Some(payload),
            done: None,
            total: None,
            file_name: None,
            size_bytes: None,
            saved_path: None,
            latency_ms: None,
            content_kind: None,
            item_count: None,
        },
        TransferEvent::HandshakeCode(code) => TransferEventRecord {
            kind: "handshake_code".to_string(),
            message: None,
            value: Some(code),
            done: None,
            total: None,
            file_name: None,
            size_bytes: None,
            saved_path: None,
            latency_ms: None,
            content_kind: None,
            item_count: None,
        },
        TransferEvent::Progress { done, total } => TransferEventRecord {
            kind: "progress".to_string(),
            message: None,
            value: None,
            done: Some(done),
            total: Some(total),
            file_name: None,
            size_bytes: None,
            saved_path: None,
            latency_ms: None,
            content_kind: None,
            item_count: None,
        },
        TransferEvent::ConnectionPath { kind, latency_ms } => {
            let (value, message) = match kind {
                ConnectionPathKind::Direct(addr) => (Some("direct".to_string()), Some(addr)),
                ConnectionPathKind::Relay(url) => (Some("relay".to_string()), Some(url)),
                ConnectionPathKind::Mixed {
                    udp_addr,
                    relay_url,
                } => (
                    Some("mixed".to_string()),
                    Some(format!("udp: {}, relay: {}", udp_addr, relay_url)),
                ),
                ConnectionPathKind::None => (Some("none".to_string()), None),
            };

            TransferEventRecord {
                kind: "connection_path".to_string(),
                message,
                value,
                done: None,
                total: None,
                file_name: None,
                size_bytes: None,
                saved_path: None,
                latency_ms,
                content_kind: None,
                item_count: None,
            }
        }
        TransferEvent::Completed(result) => TransferEventRecord {
            kind: "completed".to_string(),
            message: None,
            value: None,
            done: None,
            total: None,
            file_name: Some(result.file_name),
            size_bytes: Some(result.size_bytes),
            saved_path: result.saved_path.map(|p| p.display().to_string()),
            latency_ms: None,
            content_kind: Some(match result.content_kind {
                TransferContentKind::File => "file".to_string(),
                TransferContentKind::Bundle => "bundle".to_string(),
            }),
            item_count: Some(result.item_count),
        },
        TransferEvent::Offer(offer) => TransferEventRecord {
            kind: "offer".to_string(),
            message: offer.sender_name,
            file_name: Some(offer.file_name),
            size_bytes: Some(offer.size_bytes),
            item_count: Some(offer.item_count),
            content_kind: Some(match offer.content_kind {
                TransferContentKind::File => "file".to_string(),
                TransferContentKind::Bundle => "bundle".to_string(),
            }),
            ..TransferEventRecord::status("")
        },
        TransferEvent::Beacon(beacon) => TransferEventRecord {
            kind: "beacon".to_string(),
            message: None,
            value: Some(beacon),
            ..TransferEventRecord::status("")
        },
        TransferEvent::Error { code, message } => TransferEventRecord::error(code, message),
    }
}

fn with_controller<F>(handle: u64, f: F)
where
    F: FnOnce(&TransferController),
{
    if handle == 0 {
        return;
    }
    // SAFETY: handle is created from Box<TransferController> in p2pshare_controller_create.
    let controller = unsafe { &*(handle as *const TransferController) };
    f(controller);
}

fn cstr_to_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: caller must provide a valid NUL-terminated C string pointer.
    let s = unsafe { CStr::from_ptr(ptr) }.to_str().ok()?;
    Some(s.to_string())
}

#[cfg(target_os = "android")]
unsafe extern "C" {
    fn p2pshare_jni_register(vm: *mut c_void) -> i32;
}

/// Install the process-wide application context before creating endpoints.
///
/// # Safety
/// Both pointers must remain valid for the process lifetime; call exactly once.
#[cfg(target_os = "android")]
#[no_mangle]
pub unsafe extern "C" fn p2pshare_android_initialize(vm: *mut c_void, context: *mut c_void) {
    // The C bridge retains one application global reference for the process lifetime.
    unsafe {
        iroh::dns::install_android_jni_context(vm, context);
    }
}

#[cfg(target_os = "android")]
#[no_mangle]
pub extern "C" fn JNI_OnLoad(vm: *mut c_void, _reserved: *mut c_void) -> i32 {
    // SAFETY: called by JVM with valid JavaVM pointer during library load.
    unsafe { p2pshare_jni_register(vm) }
}

#[no_mangle]
pub extern "C" fn p2pshare_controller_create() -> u64 {
    let controller = Box::new(TransferController::new());
    Box::into_raw(controller) as u64
}

/// The caller must stop using the handle before destroying it, exactly once.
#[no_mangle]
pub extern "C" fn p2pshare_controller_destroy(handle: u64) {
    if handle == 0 {
        return;
    }
    // SAFETY: JNI serializes access and clears its handle before this call.
    let controller = unsafe { Box::from_raw(handle as *mut TransferController) };
    std::thread::spawn(move || controller.shutdown());
}

#[no_mangle]
pub extern "C" fn p2pshare_controller_start_send_wait(handle: u64, file_paths_json: *const c_char) {
    let Some(file_paths_json) = cstr_to_string(file_paths_json) else {
        return;
    };
    let Ok(file_paths) = serde_json::from_str::<Vec<String>>(&file_paths_json) else {
        return;
    };
    with_controller(handle, |controller| controller.start_send_wait(file_paths));
}

#[no_mangle]
pub extern "C" fn p2pshare_controller_start_send_to_ticket(
    handle: u64,
    file_paths_json: *const c_char,
    ticket: *const c_char,
) {
    let Some(file_paths_json) = cstr_to_string(file_paths_json) else {
        return;
    };
    let Some(ticket) = cstr_to_string(ticket) else {
        return;
    };
    let Ok(file_paths) = serde_json::from_str::<Vec<String>>(&file_paths_json) else {
        return;
    };
    with_controller(handle, |controller| {
        controller.start_send_to_ticket(file_paths, ticket)
    });
}

#[no_mangle]
pub extern "C" fn p2pshare_controller_start_receive_target(
    handle: u64,
    target: *const c_char,
    output_dir: *const c_char,
) {
    let Some(target) = cstr_to_string(target) else {
        return;
    };
    let Some(output_dir) = cstr_to_string(output_dir) else {
        return;
    };
    with_controller(handle, |controller| {
        controller.start_receive_target(target, output_dir)
    });
}

#[no_mangle]
pub extern "C" fn p2pshare_controller_start_receive_listen(handle: u64, output_dir: *const c_char) {
    let Some(output_dir) = cstr_to_string(output_dir) else {
        return;
    };
    with_controller(handle, |controller| {
        controller.start_receive_listen(output_dir)
    });
}

#[no_mangle]
pub extern "C" fn p2pshare_controller_start_receive_nearby(handle: u64, output_dir: *const c_char) {
    let Some(output_dir) = cstr_to_string(output_dir) else {
        return;
    };
    with_controller(handle, |controller| {
        controller.start_receive_nearby(output_dir)
    });
}

#[no_mangle]
pub extern "C" fn p2pshare_controller_set_device_name(handle: u64, name: *const c_char) {
    let Some(name) = cstr_to_string(name) else {
        return;
    };
    with_controller(handle, |controller| controller.set_device_name(name));
}

#[no_mangle]
pub extern "C" fn p2pshare_controller_answer_offer(handle: u64, accept: bool) -> bool {
    let mut answered = false;
    with_controller(handle, |controller| {
        answered = controller.answer_offer(accept)
    });
    answered
}

#[no_mangle]
pub extern "C" fn p2pshare_controller_has_relay(handle: u64) -> bool {
    let mut relay = false;
    with_controller(handle, |controller| relay = controller.has_relay());
    relay
}

#[no_mangle]
pub extern "C" fn p2pshare_controller_rebind(handle: u64) {
    with_controller(handle, TransferController::rebind);
}

#[no_mangle]
pub extern "C" fn p2pshare_controller_start_browse(handle: u64) {
    with_controller(handle, TransferController::start_browse);
}

#[no_mangle]
pub extern "C" fn p2pshare_controller_stop_browse(handle: u64) {
    with_controller(handle, TransferController::stop_browse);
}

#[no_mangle]
pub extern "C" fn p2pshare_controller_poll_nearby_json(handle: u64) -> *mut c_char {
    let mut out: Option<String> = None;
    with_controller(handle, |controller| {
        out = controller.poll_nearby_json();
    });
    match out {
        Some(json) => CString::new(json)
            .map(CString::into_raw)
            .unwrap_or(std::ptr::null_mut()),
        None => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "C" fn p2pshare_controller_poll_event_json(handle: u64) -> *mut c_char {
    let mut out: Option<String> = None;
    with_controller(handle, |controller| {
        out = controller.poll_event_json();
    });
    match out {
        Some(json) => CString::new(json)
            .map(CString::into_raw)
            .unwrap_or(std::ptr::null_mut()),
        None => std::ptr::null_mut(),
    }
}

#[no_mangle]
pub extern "C" fn p2pshare_controller_cancel(handle: u64) {
    with_controller(handle, TransferController::cancel);
}

#[no_mangle]
pub extern "C" fn p2pshare_free_cstring(ptr: *const c_char) {
    if ptr.is_null() {
        return;
    }
    // SAFETY: ptr must be returned by CString::into_raw in this crate.
    unsafe {
        let _ = CString::from_raw(ptr as *mut c_char);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_coalesces_progress_and_bounds_status_backlog() {
        let queue = Arc::new(Mutex::new(EventQueue::default()));
        for done in 0..1000 {
            push_event(
                &queue,
                0,
                map_event(TransferEvent::Progress { done, total: 1000 }),
            );
        }
        assert_eq!(queue.lock().unwrap().events.len(), 1);
        assert_eq!(queue.lock().unwrap().events[0].done, Some(999));
        push_event(&queue, 0, TransferEventRecord::error("test", "terminal"));
        for _ in 0..1000 {
            push_event(&queue, 0, TransferEventRecord::status("status"));
        }
        let queue = queue.lock().unwrap();
        assert_eq!(queue.events.len(), MAX_QUEUED_EVENTS);
        assert!(queue.events.iter().any(|event| event.kind == "error"));
    }

    #[test]
    fn destroying_controller_releases_pending_task() {
        struct OnDrop(std::sync::mpsc::Sender<()>);
        impl Drop for OnDrop {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        let signal = OnDrop(sender);
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let task = runtime.spawn(async move {
            let _signal = signal;
            std::future::pending::<()>().await;
        });
        let controller = TransferController {
            runtime,
            engine: Mutex::new(Arc::new(OnceCell::new())),
            device_name: Arc::new(Mutex::new(None)),
            queue: Arc::new(Mutex::new(EventQueue::default())),
            task: Mutex::new(Some(task)),
            browse: Arc::new(Mutex::new(BrowseState::default())),
            nearby: Arc::new(Mutex::new(VecDeque::new())),
        };
        p2pshare_controller_destroy(Box::into_raw(Box::new(controller)) as u64);
        receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("native task released");
    }

    #[test]
    fn canceled_generation_cannot_deliver_stale_events() {
        let queue = Arc::new(Mutex::new(EventQueue {
            generation: 1,
            ..Default::default()
        }));
        push_event(&queue, 0, TransferEventRecord::error("old", "old transfer"));
        push_event(&queue, 1, TransferEventRecord::status("current transfer"));
        assert_eq!(queue.lock().unwrap().events.len(), 1);
        assert_eq!(
            queue.lock().unwrap().events[0].message.as_deref(),
            Some("current transfer")
        );
    }
}
