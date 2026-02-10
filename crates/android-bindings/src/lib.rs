use std::collections::VecDeque;
#[cfg(target_os = "android")]
use std::ffi::c_void;
use std::ffi::{c_char, CStr, CString};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use p2p_share_core::events::{ConnectionPathKind, TransferEvent, TransferEventSink};
use serde::{Deserialize, Serialize};
use tokio::runtime::{Builder, Runtime};
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
        }
    }
}

struct QueueSink {
    queue: Arc<Mutex<VecDeque<TransferEventRecord>>>,
}

impl TransferEventSink for QueueSink {
    fn on_event(&self, event: TransferEvent) {
        push_event(&self.queue, map_event(event));
    }
}

pub struct TransferController {
    runtime: Runtime,
    queue: Arc<Mutex<VecDeque<TransferEventRecord>>>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl TransferController {
    pub fn new() -> Self {
        let runtime = Builder::new_multi_thread()
            .enable_all()
            .thread_name("p2p-share-runtime")
            .build()
            .expect("failed to build tokio runtime");

        Self {
            runtime,
            queue: Arc::new(Mutex::new(VecDeque::new())),
            task: Mutex::new(None),
        }
    }

    pub fn start_send_wait(&self, file_path: impl Into<String>) {
        let file_path = PathBuf::from(file_path.into());
        let queue = self.queue.clone();
        self.start_task(async move {
            p2p_share_core::sender::run_with_sink(
                file_path.as_path(),
                Some(Arc::new(QueueSink { queue }) as Arc<dyn TransferEventSink>),
            )
            .await
        });
    }

    pub fn start_send_to_ticket(&self, file_path: impl Into<String>, ticket: impl Into<String>) {
        let file_path = PathBuf::from(file_path.into());
        let ticket = ticket.into();
        let queue = self.queue.clone();
        self.start_task(async move {
            p2p_share_core::sender::run_reverse_with_sink(
                file_path.as_path(),
                &ticket,
                Some(Arc::new(QueueSink { queue }) as Arc<dyn TransferEventSink>),
            )
            .await
        });
    }

    pub fn start_receive_target(&self, target: impl Into<String>, output_dir: impl Into<String>) {
        let target = target.into();
        let output_dir = PathBuf::from(output_dir.into());
        let queue = self.queue.clone();
        self.start_task(async move {
            p2p_share_core::receiver::run_with_sink(
                &target,
                output_dir.as_path(),
                Some(Arc::new(QueueSink { queue }) as Arc<dyn TransferEventSink>),
            )
            .await
        });
    }

    pub fn start_receive_listen(&self, output_dir: impl Into<String>) {
        let output_dir = PathBuf::from(output_dir.into());
        let queue = self.queue.clone();
        self.start_task(async move {
            p2p_share_core::receiver::run_listen_with_sink(
                output_dir.as_path(),
                Some(Arc::new(QueueSink { queue }) as Arc<dyn TransferEventSink>),
            )
            .await
        });
    }

    pub fn poll_event(&self) -> Option<TransferEventRecord> {
        let mut queue = self.queue.lock().ok()?;
        queue.pop_front()
    }

    pub fn poll_event_json(&self) -> Option<String> {
        self.poll_event()
            .and_then(|evt| serde_json::to_string(&evt).ok())
    }

    pub fn cancel(&self) {
        if let Ok(mut task) = self.task.lock() {
            if let Some(handle) = task.take() {
                handle.abort();
                push_event(
                    &self.queue,
                    TransferEventRecord::status("Transfer canceled by user."),
                );
            }
        }
    }

    fn start_task<F>(&self, fut: F)
    where
        F: std::future::Future<Output = Result<()>> + Send + 'static,
    {
        self.cancel();

        let queue = self.queue.clone();
        push_event(&queue, TransferEventRecord::status("Transfer started."));

        let task = self.runtime.spawn(async move {
            if let Err(err) = fut.await {
                push_event(
                    &queue,
                    TransferEventRecord::error("transfer_error", format!("{:#}", err)),
                );
            }
        });

        if let Ok(mut current) = self.task.lock() {
            *current = Some(task);
        }
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

fn push_event(queue: &Arc<Mutex<VecDeque<TransferEventRecord>>>, event: TransferEventRecord) {
    if let Ok(mut q) = queue.lock() {
        q.push_back(event);
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

#[no_mangle]
pub extern "C" fn p2pshare_controller_start_send_wait(handle: u64, file_path: *const c_char) {
    let Some(file_path) = cstr_to_string(file_path) else {
        return;
    };
    with_controller(handle, |controller| controller.start_send_wait(file_path));
}

#[no_mangle]
pub extern "C" fn p2pshare_controller_start_send_to_ticket(
    handle: u64,
    file_path: *const c_char,
    ticket: *const c_char,
) {
    let Some(file_path) = cstr_to_string(file_path) else {
        return;
    };
    let Some(ticket) = cstr_to_string(ticket) else {
        return;
    };
    with_controller(handle, |controller| {
        controller.start_send_to_ticket(file_path, ticket)
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
