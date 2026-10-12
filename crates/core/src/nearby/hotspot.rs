//! Joining another device's hotspot through NetworkManager (`nmcli`).

use std::time::Duration;

use anyhow::{bail, Result};
use tokio::process::Command;

/// The temporary connection profile; deleting it lets NetworkManager return
/// to the previous network.
pub(super) struct Joined {
    profile: String,
}

impl Drop for Joined {
    fn drop(&mut self) {
        let profile = std::mem::take(&mut self.profile);
        std::thread::spawn(move || {
            let _ = std::process::Command::new("nmcli")
                .args(["connection", "delete", "id", &profile])
                .output();
        });
    }
}

pub(super) async fn join(ssid: &str, passphrase: &str) -> Result<Joined> {
    let profile = format!("p2p-share {ssid}");
    let mut last_error = String::new();
    // A hotspot that just started may not be in the scan results yet.
    for _ in 0..5 {
        let _ = Command::new("nmcli")
            .args(["device", "wifi", "rescan", "ssid", ssid])
            .output()
            .await;
        let output = Command::new("nmcli")
            .args(["--wait", "20", "device", "wifi", "connect", ssid])
            .args(["password", passphrase, "name", &profile])
            .output()
            .await?;
        if output.status.success() {
            return Ok(Joined { profile });
        }
        last_error = String::from_utf8_lossy(&output.stderr).trim().to_string();
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    bail!("Could not join hotspot \"{ssid}\": {last_error}")
}
