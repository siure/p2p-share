use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub enum TransferMode {
    SendWait,
    SendToTicket,
    ReceiveFromTarget,
    ReceiveListen,
}

#[derive(Debug, Clone)]
pub enum ConnectionPathKind {
    Direct(String),
    Relay(String),
    Mixed { udp_addr: String, relay_url: String },
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransferContentKind {
    File,
    Bundle,
}

#[derive(Debug, Clone)]
pub struct TransferCompleted {
    pub file_name: String,
    pub size_bytes: u64,
    pub saved_path: Option<PathBuf>,
    pub content_kind: TransferContentKind,
    pub item_count: u64,
}

#[derive(Debug, Clone)]
pub enum TransferEvent {
    Status(String),
    Ticket(String),
    QrPayload(String),
    HandshakeCode(String),
    Progress {
        done: u64,
        total: u64,
    },
    ConnectionPath {
        kind: ConnectionPathKind,
        latency_ms: Option<f64>,
    },
    Completed(TransferCompleted),
    Error {
        code: String,
        message: String,
    },
}

pub trait TransferEventSink: Send + Sync {
    fn on_event(&self, event: TransferEvent);
}

impl<F> TransferEventSink for F
where
    F: Fn(TransferEvent) + Send + Sync,
{
    fn on_event(&self, event: TransferEvent) {
        self(event)
    }
}
