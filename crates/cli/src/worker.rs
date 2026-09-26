//! A single endpoint serves sequential desktop transfers until stdin closes.

use std::io::BufRead;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use p2p_share_core::TransferEngine;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::{emit_json_line, StdoutJsonSink, TransferEventRecord};

#[derive(Debug, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
enum WorkerCommand {
    Send {
        id: String,
        files: Vec<PathBuf>,
        to: Option<String>,
    },
    Receive {
        id: String,
        target: Option<String>,
        output: PathBuf,
    },
    Cancel {
        id: String,
    },
    Shutdown,
}

struct ActiveTransfer {
    id: String,
    sink: Arc<StdoutJsonSink>,
    task: JoinHandle<Result<()>>,
}

impl ActiveTransfer {
    fn finish(&self, result: Result<Result<()>, tokio::task::JoinError>) {
        let outcome = match result {
            Ok(Ok(())) => "completed",
            Err(err) if err.is_cancelled() => "canceled",
            result => {
                let message = match result {
                    Ok(Err(err)) => format!("{err:#}"),
                    Err(err) => format!("Transfer task failed: {err}"),
                    Ok(Ok(())) => unreachable!(),
                };
                self.sink
                    .emit(TransferEventRecord::error("transfer_error", message));
                "failed"
            }
        };
        let mut event = TransferEventRecord::base("process_end");
        event.value = Some(outcome.to_string());
        self.sink.emit(event);
    }

    async fn cancel(mut self) {
        self.task.abort();
        let result = (&mut self.task).await;
        self.finish(result);
    }
}

pub async fn run() -> Result<()> {
    // Tokio's stdin uses an uncancelable blocking read. A detached reader lets
    // explicit shutdown finish even when the parent still has stdin open.
    let (tx, mut commands) = mpsc::channel(16);
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if tx.blocking_send(line).is_err() {
                break;
            }
        }
    });

    let engine = Arc::new(TransferEngine::bind().await?);
    emit_json_line(&TransferEventRecord::base("worker_ready"));
    let mut active: Option<ActiveTransfer> = None;

    loop {
        tokio::select! {
            biased;
            result = async {
                match active.as_mut() {
                    Some(transfer) => (&mut transfer.task).await,
                    None => std::future::pending().await,
                }
            } => {
                if let Some(transfer) = active.take() {
                    transfer.finish(result);
                }
            }
            line = commands.recv() => {
                let Some(line) = line else { break };
                if line.trim().is_empty() { continue; }
                let command: WorkerCommand = match serde_json::from_str(&line) {
                    Ok(command) => command,
                    Err(err) => {
                        emit_json_line(&TransferEventRecord::error("worker_command", format!("Invalid worker command: {err}")));
                        continue;
                    }
                };
                match command {
                    WorkerCommand::Shutdown => break,
                    WorkerCommand::Cancel { id } => {
                        if active.as_ref().is_some_and(|transfer| transfer.id == id) {
                            active.take().expect("active transfer checked").cancel().await;
                        }
                    }
                    command => {
                        let id = match &command {
                            WorkerCommand::Send { id, .. } | WorkerCommand::Receive { id, .. } => id.clone(),
                            _ => unreachable!(),
                        };
                        let sink = Arc::new(StdoutJsonSink::for_transfer(id.clone()));
                        if active.as_ref().is_some_and(|transfer| transfer.id == id) {
                            emit_json_line(&TransferEventRecord::error("worker_command", "Transfer id is already active."));
                            continue;
                        }
                        if id.is_empty() || active.is_some() {
                            let message = if id.is_empty() { "Transfer id must not be empty." } else { "A transfer is already running." };
                            sink.emit(TransferEventRecord::error("worker_command", message));
                            let mut event = TransferEventRecord::base("process_end");
                            event.value = Some("failed".to_string());
                            sink.emit(event);
                            continue;
                        }
                        let transfer_engine = engine.clone();
                        let transfer_sink = sink.clone();
                        sink.emit(TransferEventRecord::status("Transfer started."));
                        let task = tokio::spawn(async move {
                            match command {
                                WorkerCommand::Send { files, to, .. } => transfer_engine.send_paths(&files, to.as_deref(), Some(transfer_sink)).await,
                                WorkerCommand::Receive { target, output, .. } => transfer_engine.receive(target.as_deref(), &output, Some(transfer_sink)).await,
                                _ => unreachable!(),
                            }
                        });
                        active = Some(ActiveTransfer { id, sink, task });
                    }
                }
            }
        }
    }

    if let Some(transfer) = active.take() {
        transfer.cancel().await;
    }
    engine.close().await;
    Ok(())
}
