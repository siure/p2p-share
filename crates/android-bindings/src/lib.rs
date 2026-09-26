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

pub struct TransferController {
    runtime: Runtime,
    engine: Arc<OnceCell<TransferEngine>>,
    queue: Arc<Mutex<EventQueue>>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl TransferController {
    pub fn new() -> Self {
        let runtime = Builder::new_multi_thread()
            .enable_all()
            .thread_name("p2p-share-runtime")
            .build()
            .expect("failed to build tokio runtime");
        let engine = Arc::new(OnceCell::new());
        let warm_engine = engine.clone();
        runtime.spawn(async move {
            let _ = warm_engine.get_or_try_init(TransferEngine::bind).await;
        });
        Self {
            runtime,
            engine,
            queue: Arc::new(Mutex::new(EventQueue::default())),
            task: Mutex::new(None),
        }
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
        F: FnOnce(Arc<OnceCell<TransferEngine>>, SharedSink) -> Fut + Send + 'static,
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
        let engine = self.engine.clone();
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
        self.runtime.block_on(async {
            if let Some(engine) = self.engine.get() {
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
            engine: Arc::new(OnceCell::new()),
            queue: Arc::new(Mutex::new(EventQueue::default())),
            task: Mutex::new(Some(task)),
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
