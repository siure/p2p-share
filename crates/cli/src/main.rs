use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::{io, io::Write};

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use p2p_share_core::events::{ConnectionPathKind, TransferEvent, TransferEventSink};
use serde::Serialize;

const TRANSFER_EVENT_SCHEMA_VERSION: &str = "1.0.0";

/// p2p-share — simple peer-to-peer file transfer.
///
/// Uses iroh for automatic NAT traversal (UPnP, hole-punching, relay),
/// so no manual port opening is required.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Emit structured transfer events as JSON lines on stdout.
    /// Useful for integrations such as desktop GUI frontends.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Send a file to another device.
    Send {
        /// Path to the file to send.
        file: PathBuf,

        /// Connect to a waiting receiver instead of waiting for one.
        /// Use the ticket shown by `p2p-share receive --qr`.
        #[arg(long)]
        to: Option<String>,
    },

    /// Receive a file from another device.
    Receive {
        /// Connection ticket (shown by the sender) or ip:port for direct LAN.
        /// Not required when using --qr.
        target: Option<String>,

        /// Directory to save the received file in.
        #[arg(short, long, default_value = ".")]
        output: PathBuf,

        /// Listen mode: create an endpoint, display a QR code, and wait for
        /// a sender to connect with `p2p-share send --to <ticket>`.
        /// Useful when the sender is a phone and typing long tickets is impractical.
        #[arg(long)]
        qr: bool,
    },

    /// Print machine-readable version metadata.
    Version,
}

#[derive(Debug, Clone, Serialize)]
struct TransferEventRecord {
    kind: String,
    message: Option<String>,
    value: Option<String>,
    schema_version: String,
    done: Option<u64>,
    total: Option<u64>,
    file_name: Option<String>,
    size_bytes: Option<u64>,
    saved_path: Option<String>,
    latency_ms: Option<f64>,
}

impl TransferEventRecord {
    fn base(kind: impl Into<String>) -> Self {
        Self {
            kind: kind.into(),
            message: None,
            value: None,
            schema_version: TRANSFER_EVENT_SCHEMA_VERSION.to_string(),
            done: None,
            total: None,
            file_name: None,
            size_bytes: None,
            saved_path: None,
            latency_ms: None,
        }
    }

    fn status(message: impl Into<String>) -> Self {
        let mut record = Self::base("status");
        record.message = Some(message.into());
        record
    }

    fn error(code: impl Into<String>, message: impl Into<String>) -> Self {
        let mut record = Self::base("error");
        record.message = Some(message.into());
        record.value = Some(code.into());
        record
    }
}

#[derive(Default)]
struct StdoutJsonSink {
    write_lock: Mutex<()>,
}

impl TransferEventSink for StdoutJsonSink {
    fn on_event(&self, event: TransferEvent) {
        let _guard = self.write_lock.lock();
        emit_json_line(&map_event(event));
    }
}

fn emit_json_line(event: &TransferEventRecord) {
    if let Ok(json) = serde_json::to_string(event) {
        let mut out = io::stdout().lock();
        let _ = writeln!(out, "{json}");
        let _ = out.flush();
    }
}

fn map_event(event: TransferEvent) -> TransferEventRecord {
    match event {
        TransferEvent::Status(message) => TransferEventRecord::status(message),
        TransferEvent::Ticket(ticket) => TransferEventRecord {
            value: Some(ticket),
            ..TransferEventRecord::base("ticket")
        },
        TransferEvent::QrPayload(payload) => TransferEventRecord {
            value: Some(payload),
            ..TransferEventRecord::base("qr_payload")
        },
        TransferEvent::HandshakeCode(code) => TransferEventRecord {
            value: Some(code),
            ..TransferEventRecord::base("handshake_code")
        },
        TransferEvent::Progress { done, total } => TransferEventRecord {
            done: Some(done),
            total: Some(total),
            ..TransferEventRecord::base("progress")
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
                    Some(format!("udp: {udp_addr}, relay: {relay_url}")),
                ),
                ConnectionPathKind::None => (Some("none".to_string()), None),
            };

            TransferEventRecord {
                message,
                value,
                latency_ms,
                ..TransferEventRecord::base("connection_path")
            }
        }
        TransferEvent::Completed(result) => TransferEventRecord {
            file_name: Some(result.file_name),
            size_bytes: Some(result.size_bytes),
            saved_path: result.saved_path.map(|p| p.display().to_string()),
            ..TransferEventRecord::base("completed")
        },
        TransferEvent::Error { code, message } => TransferEventRecord::error(code, message),
    }
}

fn emit_version_json() -> Result<()> {
    let payload = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "schema_version": TRANSFER_EVENT_SCHEMA_VERSION,
    });
    let mut out = io::stdout().lock();
    writeln!(out, "{}", serde_json::to_string(&payload)?)?;
    out.flush()?;
    Ok(())
}

fn missing_target_error() -> anyhow::Error {
    anyhow!(
        "either provide a <TARGET> ticket/address, or use --qr to listen.\n\n\
         Examples:\n  p2p-share receive p2psh:XXXXX        # connect to a sender\n  p2p-share receive --qr               # wait for a sender (shows QR)"
    )
}

async fn run_human(command: Command) -> Result<()> {
    match command {
        Command::Version => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::Send { file, to: None } => p2p_share_core::sender::run(&file).await,
        Command::Send {
            file,
            to: Some(ticket),
        } => p2p_share_core::sender::run_reverse(&file, &ticket).await,
        Command::Receive {
            target: _,
            output,
            qr: true,
        } => p2p_share_core::receiver::run_listen(&output).await,
        Command::Receive {
            target: Some(target),
            output,
            qr: false,
        } => p2p_share_core::receiver::run(&target, &output).await,
        Command::Receive {
            target: None,
            output: _,
            qr: false,
        } => Err(missing_target_error()),
    }
}

async fn run_json(command: Command) -> Result<()> {
    if let Command::Version = &command {
        return emit_version_json();
    }

    let sink: Arc<dyn TransferEventSink> = Arc::new(StdoutJsonSink::default());
    emit_json_line(&TransferEventRecord::status("Transfer started."));

    let result = match command {
        Command::Version => unreachable!("handled above"),
        Command::Send { file, to: None } => {
            p2p_share_core::sender::run_with_sink(&file, Some(sink.clone())).await
        }
        Command::Send {
            file,
            to: Some(ticket),
        } => p2p_share_core::sender::run_reverse_with_sink(&file, &ticket, Some(sink.clone())).await,
        Command::Receive {
            target: _,
            output,
            qr: true,
        } => p2p_share_core::receiver::run_listen_with_sink(&output, Some(sink.clone())).await,
        Command::Receive {
            target: Some(target),
            output,
            qr: false,
        } => p2p_share_core::receiver::run_with_sink(&target, &output, Some(sink.clone())).await,
        Command::Receive {
            target: None,
            output: _,
            qr: false,
        } => Err(missing_target_error()),
    };

    if let Err(err) = &result {
        emit_json_line(&TransferEventRecord::error(
            "transfer_error",
            format!("{:#}", err),
        ));
    }

    result
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let result = if cli.json {
        run_json(cli.command).await
    } else {
        run_human(cli.command).await
    };

    if let Err(e) = result {
        eprintln!("Error: {:#}", e);
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::{map_event, missing_target_error, TRANSFER_EVENT_SCHEMA_VERSION};
    use p2p_share_core::events::{ConnectionPathKind, TransferCompleted, TransferEvent};
    use std::path::PathBuf;

    #[test]
    fn map_event_progress_keeps_counts() {
        let record = map_event(TransferEvent::Progress { done: 16, total: 64 });
        assert_eq!(record.kind, "progress");
        assert_eq!(record.schema_version, TRANSFER_EVENT_SCHEMA_VERSION);
        assert_eq!(record.done, Some(16));
        assert_eq!(record.total, Some(64));
        assert!(record.message.is_none());
    }

    #[test]
    fn map_event_connection_path_mixed_formats_message() {
        let record = map_event(TransferEvent::ConnectionPath {
            kind: ConnectionPathKind::Mixed {
                udp_addr: "192.168.1.2:4000".to_string(),
                relay_url: "https://relay.example".to_string(),
            },
            latency_ms: Some(21.5),
        });
        assert_eq!(record.kind, "connection_path");
        assert_eq!(record.value.as_deref(), Some("mixed"));
        assert_eq!(
            record.message.as_deref(),
            Some("udp: 192.168.1.2:4000, relay: https://relay.example")
        );
        assert_eq!(record.latency_ms, Some(21.5));
    }

    #[test]
    fn map_event_completed_includes_saved_path() {
        let record = map_event(TransferEvent::Completed(TransferCompleted {
            file_name: "demo.txt".to_string(),
            size_bytes: 42,
            saved_path: Some(PathBuf::from("/tmp/demo.txt")),
        }));
        assert_eq!(record.kind, "completed");
        assert_eq!(record.file_name.as_deref(), Some("demo.txt"));
        assert_eq!(record.size_bytes, Some(42));
        assert_eq!(record.saved_path.as_deref(), Some("/tmp/demo.txt"));
    }

    #[test]
    fn missing_target_error_includes_examples() {
        let msg = format!("{:#}", missing_target_error());
        assert!(msg.contains("either provide a <TARGET> ticket/address"));
        assert!(msg.contains("p2p-share receive p2psh:XXXXX"));
        assert!(msg.contains("p2p-share receive --qr"));
    }
}
