use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

struct Worker {
    child: Child,
    input: ChildStdin,
    events: Receiver<Value>,
}

impl Worker {
    fn start() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_p2p-share"))
            .args(["--json", "worker"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start worker");
        let input = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, events) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let event = serde_json::from_str(&line).expect("worker stdout must be JSON");
                if tx.send(event).is_err() {
                    break;
                }
            }
        });
        let worker = Self {
            child,
            input,
            events,
        };
        worker.wait(None, "worker_ready");
        worker
    }

    fn send(&mut self, command: Value) {
        writeln!(self.input, "{command}").unwrap();
        self.input.flush().unwrap();
    }

    fn wait(&self, id: Option<&str>, kind: &str) -> Value {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let event = self
                .events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|err| panic!("waiting for {id:?}/{kind}: {err}"));
            if event["kind"] == kind && event["transfer_id"].as_str() == id {
                return event;
            }
            assert_ne!(event["kind"], "error", "unexpected worker error: {event}");
        }
    }

    fn shutdown(&mut self) {
        self.send(json!({"command":"shutdown"}));
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "worker shutdown failed: {status}");
                return;
            }
            assert!(
                Instant::now() < deadline,
                "worker failed to exit while stdin remained open"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn worker_recovers_from_failure_and_cancel_without_restarting() {
    let temp = tempfile::tempdir().unwrap();
    let mut worker = Worker::start();
    let pid = worker.child.id();
    worker.send(json!({"id":"invalid", "command":"send", "files":[temp.path().join("missing")]}));
    let error = worker.wait(Some("invalid"), "error");
    assert_eq!(error["value"], "transfer_error");
    assert_eq!(
        worker.wait(Some("invalid"), "process_end")["value"],
        "failed"
    );

    for id in ["first", "second"] {
        worker.send(json!({"id":id, "command":"receive", "output":temp.path()}));
        let ticket = worker.wait(Some(id), "ticket");
        assert!(ticket["value"].as_str().unwrap().starts_with("p2psh:"));
        worker.send(json!({"id":"busy", "command":"receive", "output":temp.path()}));
        assert_eq!(
            worker.wait(Some("busy"), "error")["value"],
            "worker_command"
        );
        assert_eq!(worker.wait(Some("busy"), "process_end")["value"], "failed");
        worker.send(json!({"id":"stale", "command":"cancel"}));
        worker.send(json!({"id":id, "command":"cancel"}));
        assert_eq!(worker.wait(Some(id), "process_end")["value"], "canceled");
        assert_eq!(worker.child.id(), pid);
    }
    worker.shutdown();
}

#[test]
fn worker_reuses_endpoint_for_normal_reverse_and_bundle_transfers() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("payload.bin");
    let extra = temp.path().join("extra.txt");
    let bytes: Vec<_> = (0..262_144).map(|i| (i % 251) as u8).collect();
    std::fs::write(&input, &bytes).unwrap();
    std::fs::write(&extra, b"second bundle member").unwrap();
    let mut sender = Worker::start();
    let mut receiver = Worker::start();
    let mut endpoint_id = None;

    for (index, reverse, bundle) in [
        (0, false, false),
        (1, true, false),
        (2, false, true),
        (3, false, false),
    ] {
        let expected = if index == 3 { &[][..] } else { &bytes[..] };
        std::fs::write(&input, expected).unwrap();
        let id = index.to_string();
        let output = temp.path().join(format!("received-{index}"));
        let files = if bundle {
            vec![&input, &extra]
        } else {
            vec![&input]
        };
        if reverse {
            receiver.send(json!({"id":id,"command":"receive","output":output}));
            let ticket = receiver.wait(Some(&id), "ticket");
            sender.send(json!({"id":id,"command":"send","files":files,"to":ticket["value"]}));
        } else {
            sender.send(json!({"id":id,"command":"send","files":files}));
            let ticket = sender.wait(Some(&id), "ticket");
            // Tickets update as reachability changes, but the endpoint identity
            // must survive subsequent transfers in this worker.
            let addr =
                p2p_share_core::ticket::deserialize(ticket["value"].as_str().unwrap()).unwrap();
            if let Some(previous) = endpoint_id {
                assert_eq!(addr.id, previous);
            }
            endpoint_id = Some(addr.id);
            receiver.send(
                json!({"id":id,"command":"receive","target":ticket["value"],"output":output}),
            );
        }
        let completed = receiver.wait(Some(&id), "completed");
        let saved = std::path::PathBuf::from(completed["saved_path"].as_str().unwrap());
        let received = if bundle {
            saved.join("payload.bin")
        } else {
            saved.clone()
        };
        assert_eq!(std::fs::read(received).unwrap(), expected);
        if bundle {
            assert_eq!(
                std::fs::read(saved.join("extra.txt")).unwrap(),
                b"second bundle member"
            );
        }
        assert_eq!(
            receiver.wait(Some(&id), "process_end")["value"],
            "completed"
        );
        sender.wait(Some(&id), "completed");
        assert_eq!(sender.wait(Some(&id), "process_end")["value"], "completed");
    }
    sender.shutdown();
    receiver.shutdown();
}

impl Worker {
    /// Next worker-level record (no transfer id) of `kind`.
    fn wait_global(&self, kind: &str, matches: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let event = self
                .events
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|err| panic!("waiting for {kind}: {err}"));
            if event["kind"] == kind && event["transfer_id"].is_null() && matches(&event) {
                return event;
            }
        }
    }
}

#[test]
fn nearby_receiver_asks_before_accepting_and_reports_sender_name() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("hello.txt");
    std::fs::write(&input, b"hello nearby").unwrap();
    let output = temp.path().join("out");
    let mut sender = Worker::start();
    let mut receiver = Worker::start();
    sender.send(json!({"command":"set_name","name":"Worker laptop"}));

    for (id, accept) in [("declined", false), ("accepted", true)] {
        receiver.send(json!({"id":id,"command":"receive","output":output,"nearby":true}));
        let ticket = receiver.wait(Some(id), "ticket");
        sender.send(json!({"id":id,"command":"send","files":[&input],"to":ticket["value"]}));
        let offer = receiver.wait(Some(id), "offer");
        assert_eq!(offer["message"], "Worker laptop");
        assert_eq!(offer["file_name"], "hello.txt");
        assert_eq!(offer["size_bytes"], 12);
        receiver.send(json!({"id":id,"command":"answer","accept":accept}));
        if accept {
            receiver.wait(Some(id), "completed");
            sender.wait(Some(id), "completed");
            assert_eq!(sender.wait(Some(id), "process_end")["value"], "completed");
        } else {
            let error = sender.wait(Some(id), "error");
            assert!(error["message"].as_str().unwrap().contains("declined"));
            receiver.send(json!({"id":id,"command":"cancel"}));
            assert_eq!(receiver.wait(Some(id), "process_end")["value"], "canceled");
        }
    }
    assert_eq!(
        std::fs::read(output.join("hello.txt")).unwrap(),
        b"hello nearby"
    );
    sender.shutdown();
    receiver.shutdown();
}

#[test]
#[ignore = "needs multicast (mDNS) on a local network interface"]
fn browsing_worker_finds_and_sends_to_nearby_receiver() {
    let temp = tempfile::tempdir().unwrap();
    let input = temp.path().join("found.txt");
    std::fs::write(&input, b"found over mdns").unwrap();
    let mut sender = Worker::start();
    let mut receiver = Worker::start();
    receiver.send(json!({"command":"set_name","name":"Desk receiver"}));
    receiver
        .send(json!({"id":"r","command":"receive","output":temp.path().join("out"),"nearby":true}));
    sender.send(json!({"command":"browse"}));
    let peer = sender.wait_global("nearby_found", |e| e["message"] == "Desk receiver");
    assert_eq!(peer["lan"], true);
    sender.send(json!({"id":"s","command":"send","files":[&input],"nearby":peer["peer_id"]}));
    receiver.wait(Some("r"), "offer");
    receiver.send(json!({"id":"r","command":"answer","accept":true}));
    receiver.wait(Some("r"), "completed");
    sender.wait(Some("s"), "completed");
    sender.wait_global("nearby_lost", |e| e["peer_id"] == peer["peer_id"]);
    sender.send(json!({"command":"stop_browse"}));
    sender.shutdown();
    receiver.shutdown();
}
