//! Nearby discovery: receivers advertise their session ticket, senders browse.
//!
//! The ticket is the only thing discovery has to deliver; the transfer itself
//! is the regular iroh session. Transports:
//! - mDNS on the local network (all platforms, see [`mdns`]).
//! - Bluetooth LE on Linux with the `bluetooth` feature. Android implements the
//!   same GATT service in Kotlin; the UUIDs and [`Beacon`] JSON are the contract.
//!
//! When two devices only see each other over Bluetooth and one lacks internet,
//! the receiver can be asked over GATT to start a hotspot (Android) or to join
//! the sender's hotspot (Linux), so both end up on the same IP network.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::engine::{status, SharedSink};
use crate::ticket;

#[cfg(all(target_os = "linux", feature = "bluetooth"))]
mod ble;
#[cfg(all(target_os = "linux", feature = "bluetooth"))]
mod hotspot;
mod mdns;

/// GATT service advertised by nearby receivers.
pub const BLE_SERVICE_UUID: &str = "6d1c0001-5a7b-4f6e-9a39-2b8f0c1d7e10";
/// Read: the receiver's [`Beacon`] JSON.
pub const BLE_BEACON_UUID: &str = "6d1c0002-5a7b-4f6e-9a39-2b8f0c1d7e10";
/// Write (any value): ask the receiver to start a hotspot.
/// Read: its [`Hotspot`] JSON once started, empty before.
pub const BLE_HOTSPOT_UUID: &str = "6d1c0003-5a7b-4f6e-9a39-2b8f0c1d7e10";
/// Write ([`Hotspot`] JSON): ask the receiver to join the sender's hotspot.
pub const BLE_JOIN_UUID: &str = "6d1c0004-5a7b-4f6e-9a39-2b8f0c1d7e10";

/// GATT attribute values are limited to 512 bytes.
pub const BEACON_MAX_LEN: usize = 512;
const NAME_MAX_BYTES: usize = 48;

/// What a nearby receiver publishes about itself.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Beacon {
    #[serde(rename = "v")]
    pub version: u8,
    #[serde(rename = "n")]
    pub name: String,
    #[serde(rename = "t")]
    pub ticket: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hotspot {
    #[serde(rename = "s")]
    pub ssid: String,
    #[serde(rename = "p")]
    pub passphrase: String,
}

impl Beacon {
    /// Build a beacon that fits one GATT value.
    pub fn new(name: &str, ticket: &str) -> Result<Self> {
        let name = truncate(name.trim(), NAME_MAX_BYTES);
        let mut beacon = Self {
            version: 1,
            name,
            ticket: String::new(),
        };
        let budget = BEACON_MAX_LEN - beacon.to_json().len();
        beacon.ticket = ticket::compact(ticket, budget)?;
        Ok(beacon)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("beacon serializes")
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        let beacon: Self = serde_json::from_slice(bytes)?;
        anyhow::ensure!(beacon.version == 1, "unsupported nearby beacon version");
        Ok(beacon)
    }
}

/// A receiver found nearby. `ticket` can be passed to `send --to` directly;
/// `TransferEngine::send_nearby` also handles Bluetooth-only peers.
#[derive(Debug, Clone, PartialEq)]
pub struct NearbyPeer {
    /// Endpoint id; stable for the life of the receiver's app.
    pub id: String,
    pub name: String,
    pub ticket: String,
    pub lan: bool,
    /// Bluetooth address when the peer was seen over Bluetooth.
    pub bluetooth: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NearbyEvent {
    /// New peer, or an existing one with a changed name, ticket or transport.
    Found(NearbyPeer),
    Lost(String),
}

/// Cut `text` to at most `max` bytes without splitting a character.
fn truncate(text: &str, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

pub(crate) fn default_device_name() -> String {
    let from_env = |key| std::env::var(key).ok();
    let linux = || std::fs::read_to_string("/proc/sys/kernel/hostname").ok();
    from_env("COMPUTERNAME")
        .or_else(linux)
        .or_else(|| from_env("HOSTNAME"))
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "This device".to_string())
}

enum Via {
    Lan,
    #[cfg_attr(not(all(target_os = "linux", feature = "bluetooth")), allow(dead_code))]
    Bluetooth(String),
}

#[derive(Default)]
struct Sightings {
    lan: Option<(String, String)>,
    ble: Option<(String, String, String)>,
}

/// Merges sightings of the same endpoint from every transport into one peer.
struct PeerTable {
    own_id: String,
    peers: HashMap<String, Sightings>,
    on_event: Box<dyn Fn(NearbyEvent) + Send + Sync>,
}

impl PeerTable {
    fn seen(&mut self, via: Via, name: String, ticket: String) {
        let Ok(id) = ticket::peer_id(&ticket) else {
            return;
        };
        if id == self.own_id {
            return;
        }
        let before = self.peer(&id);
        let entry = self.peers.entry(id.clone()).or_default();
        match via {
            Via::Lan => entry.lan = Some((name, ticket)),
            Via::Bluetooth(address) => entry.ble = Some((address, name, ticket)),
        }
        let after = self.peer(&id);
        if after != before {
            if let Some(peer) = after {
                (self.on_event)(NearbyEvent::Found(peer));
            }
        }
    }

    fn gone(&mut self, id: &str, lan: bool) {
        let Some(entry) = self.peers.get_mut(id) else {
            return;
        };
        if lan {
            entry.lan = None;
        } else {
            entry.ble = None;
        }
        match self.peer(id) {
            Some(peer) => (self.on_event)(NearbyEvent::Found(peer)),
            None => {
                self.peers.remove(id);
                (self.on_event)(NearbyEvent::Lost(id.to_string()));
            }
        }
    }

    /// LAN data wins: it is fresher and its ticket was not compacted.
    fn peer(&self, id: &str) -> Option<NearbyPeer> {
        let entry = self.peers.get(id)?;
        let bluetooth = entry.ble.as_ref().map(|(address, _, _)| address.clone());
        let (name, ticket) = entry
            .lan
            .clone()
            .or_else(|| entry.ble.as_ref().map(|(_, n, t)| (n.clone(), t.clone())))?;
        Some(NearbyPeer {
            id: id.to_string(),
            name,
            ticket,
            lan: entry.lan.is_some(),
            bluetooth,
        })
    }
}

type SharedTable = Arc<Mutex<PeerTable>>;

fn lock(table: &SharedTable) -> std::sync::MutexGuard<'_, PeerTable> {
    table.lock().unwrap_or_else(|e| e.into_inner())
}

/// Browses every available transport until dropped.
pub struct Browser {
    table: SharedTable,
    _mdns: Option<mdns::Browser>,
    #[cfg(all(target_os = "linux", feature = "bluetooth"))]
    ble: tokio::task::JoinHandle<()>,
}

impl Browser {
    /// Must be called inside a Tokio runtime.
    pub(crate) fn start(
        own_id: String,
        on_event: impl Fn(NearbyEvent) + Send + Sync + 'static,
    ) -> Self {
        let table = Arc::new(Mutex::new(PeerTable {
            own_id,
            peers: HashMap::new(),
            on_event: Box::new(on_event),
        }));
        Self {
            _mdns: mdns::Browser::start(table.clone()).ok(),
            #[cfg(all(target_os = "linux", feature = "bluetooth"))]
            ble: tokio::spawn(ble::browse(table.clone())),
            table,
        }
    }

    pub fn peer(&self, id: &str) -> Option<NearbyPeer> {
        lock(&self.table).peer(id)
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        #[cfg(all(target_os = "linux", feature = "bluetooth"))]
        self.ble.abort();
    }
}

/// Publishes a receiving device on every available transport until dropped.
pub(crate) struct Advertiser {
    name: String,
    mdns: Option<mdns::Advertiser>,
    #[cfg(all(target_os = "linux", feature = "bluetooth"))]
    ble: Option<ble::Advertiser>,
}

impl Advertiser {
    /// Unavailable transports are reported as status and skipped.
    pub(crate) async fn start(name: &str, sink: Option<&SharedSink>) -> Self {
        let mdns = mdns::Advertiser::new()
            .inspect_err(|err| {
                status(
                    sink,
                    format!("Local network discovery unavailable: {err:#}"),
                )
            })
            .ok();
        Self {
            name: name.to_string(),
            mdns,
            #[cfg(all(target_os = "linux", feature = "bluetooth"))]
            ble: ble::Advertiser::start()
                .await
                .inspect_err(|err| {
                    status(sink, format!("Bluetooth discovery unavailable: {err:#}"))
                })
                .ok(),
        }
    }

    pub(crate) fn update(&self, ticket: &str, beacon: &Beacon) {
        if let Some(mdns) = &self.mdns {
            mdns.update(&self.name, ticket);
        }
        #[cfg(all(target_os = "linux", feature = "bluetooth"))]
        if let Some(ble) = &self.ble {
            ble.update(beacon);
        }
        #[cfg(not(all(target_os = "linux", feature = "bluetooth")))]
        let _ = beacon;
    }
}

/// Kept alive for the transfer when reaching the peer required joining a network.
pub(crate) type NetworkGuard = Option<Box<dyn Send>>;

/// Pick a ticket that can reach `peer`. LAN tickets work as-is. For Bluetooth-only
/// peers, both sides need internet for the relay, otherwise a hotspot is set up.
pub(crate) async fn reach(
    peer: &NearbyPeer,
    own_relay: bool,
    sink: Option<&SharedSink>,
) -> Result<(String, NetworkGuard)> {
    #[cfg(all(target_os = "linux", feature = "bluetooth"))]
    if let (false, Some(address)) = (peer.lan, &peer.bluetooth) {
        return ble::reach(address, own_relay, sink).await;
    }
    let _ = (own_relay, sink);
    Ok((peer.ticket.clone(), None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::EndpointAddr;

    fn ticket_for(key: &iroh::SecretKey, port: u16) -> String {
        let addr = EndpointAddr::new(key.public())
            .with_ip_addr(format!("192.168.1.9:{port}").parse().unwrap());
        let session = format!("p2p-share/2-nearby/{}", "c".repeat(64));
        ticket::serialize_session(&addr, Some(&session)).unwrap()
    }

    #[test]
    fn beacon_fits_one_gatt_value() {
        let key = iroh::SecretKey::generate();
        let mut addr = EndpointAddr::new(key.public())
            .with_relay_url("https://relay.example./".parse().unwrap());
        for i in 1..20 {
            addr = addr.with_ip_addr(format!("[2001:db8::{i}]:4000").parse().unwrap());
        }
        let session = format!("p2p-share/2-nearby/{}", "c".repeat(64));
        let full = ticket::serialize_session(&addr, Some(&session)).unwrap();
        let beacon = Beacon::new(&"é".repeat(60), &full).unwrap();
        let json = beacon.to_json();
        assert!(json.len() <= 512, "{} bytes", json.len());
        assert_eq!(beacon.name, "é".repeat(24));
        assert_eq!(Beacon::from_json(json.as_bytes()).unwrap(), beacon);
    }

    #[test]
    fn table_merges_transports_and_reports_loss_once_all_are_gone() {
        let events = Arc::new(Mutex::new(Vec::new()));
        let recorded = events.clone();
        let own = iroh::SecretKey::generate();
        let mut table = PeerTable {
            own_id: own.public().to_string(),
            peers: HashMap::new(),
            on_event: Box::new(move |event| recorded.lock().unwrap().push(event)),
        };
        let peer = iroh::SecretKey::generate();
        let id = peer.public().to_string();
        table.seen(Via::Lan, "Own".into(), ticket_for(&own, 1));
        table.seen(
            Via::Bluetooth("AA".into()),
            "Phone".into(),
            ticket_for(&peer, 1),
        );
        table.seen(Via::Lan, "Phone".into(), ticket_for(&peer, 2));
        table.seen(Via::Lan, "Phone".into(), ticket_for(&peer, 2));
        table.gone(&id, true);
        table.gone(&id, false);
        let events = events.lock().unwrap();
        assert_eq!(
            events.len(),
            4,
            "own id and repeats are ignored: {events:?}"
        );
        assert!(
            matches!(&events[1], NearbyEvent::Found(p) if p.lan && p.bluetooth.as_deref() == Some("AA"))
        );
        assert!(matches!(&events[2], NearbyEvent::Found(p) if !p.lan && p.bluetooth.is_some()));
        assert_eq!(events[3], NearbyEvent::Lost(id));
    }
}
