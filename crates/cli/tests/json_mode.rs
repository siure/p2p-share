use std::process::Command;

use serde_json::Value;

fn parse_json_events(stdout: &[u8]) -> Vec<Value> {
    let text = String::from_utf8(stdout.to_vec()).expect("stdout should be UTF-8");
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect()
}

#[test]
fn json_mode_emits_status_and_error_events_for_invalid_receive_invocation() {
    let output = Command::new(env!("CARGO_BIN_EXE_p2p-share"))
        .args(["--json", "receive"])
        .output()
        .expect("failed to run p2p-share");

    assert!(
        !output.status.success(),
        "receive without target should fail in json mode"
    );

    let events = parse_json_events(&output.stdout);
    assert!(
        events
            .iter()
            .any(|evt| evt.get("kind").and_then(Value::as_str) == Some("status")
                && evt.get("message").and_then(Value::as_str) == Some("Transfer started.")),
        "expected a startup status event in stdout JSON stream"
    );

    let error_event = events
        .iter()
        .find(|evt| evt.get("kind").and_then(Value::as_str) == Some("error"))
        .expect("expected an error event in stdout JSON stream");

    assert_eq!(
        error_event.get("value").and_then(Value::as_str),
        Some("transfer_error"),
        "error event should carry transfer_error code"
    );

    let message = error_event
        .get("message")
        .and_then(Value::as_str)
        .expect("error message should be present");
    assert!(
        message.contains("either provide a <TARGET> ticket/address"),
        "unexpected error message: {message}"
    );
    assert!(
        message.contains("p2p-share receive --qr"),
        "expected help examples in error message"
    );
}
