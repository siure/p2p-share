use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::{io, io::Write};

use anyhow::{anyhow, Result};
use clap::{Parser, Subcommand};
use p2p_share_core::events::{
    ConnectionPathKind, TransferContentKind, TransferEvent, TransferEventSink,
};
use p2p_share_core::nearby::{NearbyEvent, NearbyPeer};
use p2p_share_core::TransferEngine;
use serde::Serialize;

mod worker;

const TRANSFER_EVENT_SCHEMA_VERSION: &str = "1.2.0";

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

    /// Name shown to nearby devices. Defaults to the host name.
    #[arg(long, global = true)]
    name: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Send one or more files to another device.
    Send {
        /// One or more file paths to send.
        #[arg(required = true)]
        files: Vec<PathBuf>,

        /// Connect to a waiting receiver instead of waiting for one.
        /// Use the ticket shown by `p2p-share receive --qr`.
        #[arg(long)]
        to: Option<String>,

        /// Send to a nearby device by name (see `p2p-share nearby`).
        #[arg(long, conflicts_with = "to")]
        nearby: Option<String>,
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

        /// Become visible to nearby devices and ask before accepting files.
        #[arg(long, conflicts_with_all = ["target", "qr"])]
        nearby: bool,
    },

    /// List devices that are receiving nearby (`receive --nearby`).
    Nearby {
        /// How long to look for devices.
        #[arg(long, default_value_t = 5)]
        seconds: u64,
    },

    /// Print machine-readable version metadata.
    Version,

    /// Keep the transfer engine alive and accept JSON commands on stdin.
    Worker,
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
    content_kind: Option<TransferContentKind>,
    item_count: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    transfer_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    peer_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lan: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bluetooth: Option<bool>,
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
            content_kind: None,
            item_count: None,
            transfer_id: None,
            peer_id: None,
            lan: None,
            bluetooth: None,
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
    transfer_id: Option<String>,
}

impl StdoutJsonSink {
    fn for_transfer(id: String) -> Self {
        Self {
            write_lock: Mutex::new(()),
            transfer_id: Some(id),
        }
    }

    fn emit(&self, mut event: TransferEventRecord) {
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        event.transfer_id.clone_from(&self.transfer_id);
        emit_json_line(&event);
    }
}

impl TransferEventSink for StdoutJsonSink {
    fn on_event(&self, event: TransferEvent) {
        self.emit(map_event(event));
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
            content_kind: Some(result.content_kind),
            item_count: Some(result.item_count),
            ..TransferEventRecord::base("completed")
        },
        TransferEvent::Offer(offer) => TransferEventRecord {
            message: offer.sender_name,
            file_name: Some(offer.file_name),
            size_bytes: Some(offer.size_bytes),
            content_kind: Some(offer.content_kind),
            item_count: Some(offer.item_count),
            ..TransferEventRecord::base("offer")
        },
        TransferEvent::Beacon(beacon) => TransferEventRecord {
            value: Some(beacon),
            ..TransferEventRecord::base("beacon")
        },
        TransferEvent::Error { code, message } => TransferEventRecord::error(code, message),
    }
}

/// Worker-level records: frontends key peers by `peer_id`.
fn map_nearby(event: NearbyEvent) -> TransferEventRecord {
    match event {
        NearbyEvent::Found(peer) => TransferEventRecord {
            peer_id: Some(peer.id),
            message: Some(peer.name),
            value: Some(peer.ticket),
            lan: Some(peer.lan),
            bluetooth: Some(peer.bluetooth.is_some()),
            ..TransferEventRecord::base("nearby_found")
        },
        NearbyEvent::Lost(id) => TransferEventRecord {
            peer_id: Some(id),
            ..TransferEventRecord::base("nearby_lost")
        },
    }
}

/// Browse until a peer named `query` (case-insensitive) or with that id prefix appears.
async fn find_nearby(engine: &TransferEngine, query: &str) -> Result<NearbyPeer> {
    let (tx, mut found) = tokio::sync::mpsc::unbounded_channel();
    let _browser = engine.browse_nearby(move |event| {
        if let NearbyEvent::Found(peer) = event {
            let _ = tx.send(peer);
        }
    });
    let query = query.trim();
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        while let Some(peer) = found.recv().await {
            if peer.name.eq_ignore_ascii_case(query)
                || (query.len() >= 6 && peer.id.starts_with(query))
            {
                return Some(peer);
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
    .ok_or_else(|| {
        anyhow!("No nearby device named \"{query}\". Run `p2p-share nearby` to list them.")
    })
}

/// Print peers as they appear; JSON mode emits the worker's records instead.
async fn list_nearby(engine: &TransferEngine, seconds: u64, json: bool) -> Result<()> {
    if !json {
        eprintln!("Looking for nearby devices for {seconds} seconds...");
    }
    let _browser = engine.browse_nearby(move |event| match event {
        _ if json => emit_json_line(&map_nearby(event)),
        NearbyEvent::Found(peer) => {
            let via = match (peer.lan, peer.bluetooth.is_some()) {
                (true, true) => "LAN, Bluetooth",
                (true, false) => "LAN",
                _ => "Bluetooth",
            };
            println!("{}\t{via}\t{}", peer.name, peer.ticket);
        }
        NearbyEvent::Lost(_) => {}
    });
    tokio::time::sleep(std::time::Duration::from_secs(seconds)).await;
    Ok(())
}

/// Commands that need a configured long-lived engine (nearby features).
async fn run_engine(
    command: Command,
    name: Option<String>,
    sink: Option<Arc<dyn TransferEventSink>>,
) -> Result<()> {
    let engine = TransferEngine::bind().await?;
    if let Some(name) = name {
        engine.set_device_name(&name);
    }
    let result = match command {
        Command::Receive { output, .. } => engine.receive_nearby(&output, sink).await,
        Command::Send {
            files,
            nearby: Some(query),
            ..
        } => match find_nearby(&engine, &query).await {
            Ok(peer) => engine.send_nearby(&files, &peer, sink).await,
            Err(err) => Err(err),
        },
        Command::Nearby { seconds } => list_nearby(&engine, seconds, sink.is_some()).await,
        _ => unreachable!("only nearby commands use run_engine"),
    };
    engine.close().await;
    result
}

fn uses_nearby(command: &Command) -> bool {
    matches!(
        command,
        Command::Receive { nearby: true, .. }
            | Command::Send {
                nearby: Some(_),
                ..
            }
            | Command::Nearby { .. }
    )
}

fn emit_version_json() -> Result<()> {
    let payload = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "schema_version": TRANSFER_EVENT_SCHEMA_VERSION,
        "capabilities": ["worker", "nearby"],
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

async fn run_human(command: Command, name: Option<String>) -> Result<()> {
    if uses_nearby(&command) {
        return run_engine(command, name, None).await;
    }
    match command {
        Command::Worker => Err(anyhow!("worker requires --json")),
        Command::Version => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::Send {
            files, to: None, ..
        } => p2p_share_core::sender::run_paths(&files).await,
        Command::Send {
            files,
            to: Some(ticket),
            ..
        } => p2p_share_core::sender::run_reverse_paths(&files, &ticket).await,
        Command::Receive {
            output, qr: true, ..
        } => p2p_share_core::receiver::run_listen(&output).await,
        Command::Receive {
            target: Some(target),
            output,
            ..
        } => p2p_share_core::receiver::run(&target, &output).await,
        Command::Receive { target: None, .. } => Err(missing_target_error()),
        Command::Nearby { .. } => unreachable!("handled by run_engine"),
    }
}

async fn run_json(command: Command, name: Option<String>) -> Result<()> {
    if let Command::Version = &command {
        return emit_version_json();
    }
    if let Command::Worker = &command {
        return worker::run(name).await;
    }

    let sink: Arc<dyn TransferEventSink> = Arc::new(StdoutJsonSink::default());
    emit_json_line(&TransferEventRecord::status("Transfer started."));

    let result = match command {
        Command::Version | Command::Worker => unreachable!("handled above"),
        command if uses_nearby(&command) => run_engine(command, name, Some(sink.clone())).await,
        Command::Send {
            files, to: None, ..
        } => p2p_share_core::sender::run_paths_with_sink(&files, Some(sink.clone())).await,
        Command::Send {
            files,
            to: Some(ticket),
            ..
        } => {
            p2p_share_core::sender::run_reverse_paths_with_sink(&files, &ticket, Some(sink.clone()))
                .await
        }
        Command::Receive {
            output, qr: true, ..
        } => p2p_share_core::receiver::run_listen_with_sink(&output, Some(sink.clone())).await,
        Command::Receive {
            target: Some(target),
            output,
            ..
        } => p2p_share_core::receiver::run_with_sink(&target, &output, Some(sink.clone())).await,
        Command::Receive { target: None, .. } => Err(missing_target_error()),
        Command::Nearby { .. } => unreachable!("handled by run_engine"),
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
        run_json(cli.command, cli.name).await
    } else {
        run_human(cli.command, cli.name).await
    };

    if let Err(e) = result {
        eprintln!("Error: {:#}", e);
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::{map_event, missing_target_error, Cli, Command, TRANSFER_EVENT_SCHEMA_VERSION};
    use clap::Parser;
    use p2p_share_core::events::{
        ConnectionPathKind, TransferCompleted, TransferContentKind, TransferEvent,
    };
    use std::path::PathBuf;

    #[test]
    fn map_event_progress_keeps_counts() {
        let record = map_event(TransferEvent::Progress {
            done: 16,
            total: 64,
        });
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
            content_kind: TransferContentKind::Bundle,
            item_count: 3,
        }));
        assert_eq!(record.kind, "completed");
        assert_eq!(record.file_name.as_deref(), Some("demo.txt"));
        assert_eq!(record.size_bytes, Some(42));
        assert_eq!(record.saved_path.as_deref(), Some("/tmp/demo.txt"));
        assert_eq!(record.content_kind, Some(TransferContentKind::Bundle));
        assert_eq!(record.item_count, Some(3));
    }

    #[test]
    fn missing_target_error_includes_examples() {
        let msg = format!("{:#}", missing_target_error());
        assert!(msg.contains("either provide a <TARGET> ticket/address"));
        assert!(msg.contains("p2p-share receive p2psh:XXXXX"));
        assert!(msg.contains("p2p-share receive --qr"));
    }

    #[test]
    fn send_command_accepts_multiple_files() {
        let cli = Cli::try_parse_from(["p2p-share", "send", "a.txt", "b.txt"]).expect("parse");
        match cli.command {
            Command::Send { files, to, nearby } => {
                assert_eq!(files, vec![PathBuf::from("a.txt"), PathBuf::from("b.txt")]);
                assert!(to.is_none() && nearby.is_none());
            }
            other => panic!("unexpected command: {:?}", other),
        }
    }
}
