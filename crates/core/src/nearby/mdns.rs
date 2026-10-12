//! mDNS (DNS-SD) transport. The ticket is too long for one TXT string, so it
//! is split across `t0`, `t1`, ... properties; `n` holds the device name.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;

use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

use super::{lock, SharedTable, Via};
use crate::ticket;

pub(super) const SERVICE_TYPE: &str = "_p2psh._udp.local.";
const CHUNK: usize = 200;

pub(super) struct Advertiser {
    daemon: ServiceDaemon,
    fullname: Mutex<Option<String>>,
}

impl Advertiser {
    pub(super) fn new() -> Result<Self> {
        Ok(Self {
            daemon: ServiceDaemon::new()?,
            fullname: Mutex::new(None),
        })
    }

    /// Registering again with the same instance name replaces the record.
    pub(super) fn update(&self, name: &str, ticket: &str) {
        let Ok(id) = ticket::peer_id(ticket) else {
            return;
        };
        let short = &id[..12.min(id.len())];
        // Instance labels are limited to 63 bytes; the id suffix keeps them unique.
        let label: String = name.chars().take(40).collect();
        let mut properties = vec![
            ("v".to_string(), "1".to_string()),
            ("n".to_string(), label.clone()),
        ];
        for (i, chunk) in ticket.as_bytes().chunks(CHUNK).enumerate() {
            properties.push((format!("t{i}"), String::from_utf8_lossy(chunk).into_owned()));
        }
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            &format!("{label} {short}"),
            &format!("p2psh-{short}.local."),
            &[] as &[IpAddr],
            1,
            &properties[..],
        );
        if let Ok(info) = info {
            let info = info.enable_addr_auto();
            *self.fullname.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(info.get_fullname().to_string());
            let _ = self.daemon.register(info);
        }
    }
}

impl Drop for Advertiser {
    fn drop(&mut self) {
        let fullname = self
            .fullname
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        let daemon = self.daemon.clone();
        // Send the goodbye packet before stopping, off the async runtime.
        std::thread::spawn(move || {
            if let Some(Ok(done)) = fullname.map(|name| daemon.unregister(&name)) {
                let _ = done.recv_timeout(std::time::Duration::from_secs(1));
            }
            let _ = daemon.shutdown();
        });
    }
}

pub(super) struct Browser {
    daemon: ServiceDaemon,
}

impl Browser {
    pub(super) fn start(table: SharedTable) -> Result<Self> {
        let daemon = ServiceDaemon::new()?;
        let events = daemon.browse(SERVICE_TYPE)?;
        tokio::spawn(async move {
            // Removal events only carry the instance name.
            let mut ids: HashMap<String, String> = HashMap::new();
            while let Ok(event) = events.recv_async().await {
                match event {
                    ServiceEvent::ServiceResolved(service) => {
                        let Some((name, ticket)) =
                            parse(|key| service.get_property_val_str(key).map(str::to_owned))
                        else {
                            continue;
                        };
                        if let Ok(id) = ticket::peer_id(&ticket) {
                            ids.insert(service.get_fullname().to_string(), id);
                            lock(&table).seen(Via::Lan, name, ticket);
                        }
                    }
                    ServiceEvent::ServiceRemoved(_, fullname) => {
                        if let Some(id) = ids.remove(&fullname) {
                            lock(&table).gone(&id, true);
                        }
                    }
                    _ => {}
                }
            }
        });
        Ok(Self { daemon })
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        // Ends the event stream, which stops the task above.
        let _ = self.daemon.shutdown();
    }
}

/// Reassemble `(name, ticket)` from TXT properties.
fn parse(property: impl Fn(&str) -> Option<String>) -> Option<(String, String)> {
    if property("v").as_deref() != Some("1") {
        return None;
    }
    let name = property("n").filter(|name| !name.trim().is_empty())?;
    let ticket: String = (0..).map_while(|i| property(&format!("t{i}"))).collect();
    ticket::is_ticket(&ticket).then_some((name, ticket))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rejoins_chunks_and_rejects_foreign_records() {
        let props: HashMap<&str, &str> =
            [("v", "1"), ("n", "Desk"), ("t0", "p2psh:ab"), ("t1", "cd")].into();
        let get = |key: &str| props.get(key).map(|v| v.to_string());
        assert_eq!(parse(get), Some(("Desk".into(), "p2psh:abcd".into())));
        let foreign = |key: &str| (key == "v").then(|| "2".to_string());
        assert_eq!(parse(foreign), None);
    }

    /// Two daemons on this host see each other through multicast loopback.
    #[tokio::test]
    #[ignore = "needs a multicast-capable network interface"]
    async fn browser_finds_advertised_receiver() {
        let key = iroh::SecretKey::generate();
        let addr =
            iroh::EndpointAddr::new(key.public()).with_ip_addr("192.168.1.9:4000".parse().unwrap());
        let session = format!("p2p-share/2-nearby/{}", "d".repeat(64));
        let ticket = ticket::serialize_session(&addr, Some(&session)).unwrap();
        let advertiser = Advertiser::new().unwrap();
        advertiser.update("Test receiver", &ticket);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let browser = super::super::Browser::start("self".into(), move |event| {
            let _ = tx.send(event);
        });
        let found = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
            .await
            .expect("peer discovered")
            .unwrap();
        assert!(matches!(found, super::super::NearbyEvent::Found(ref p)
            if p.name == "Test receiver" && p.ticket == ticket && p.lan));
        drop(advertiser);
        let lost = tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv())
            .await
            .expect("goodbye received")
            .unwrap();
        assert_eq!(
            lost,
            super::super::NearbyEvent::Lost(key.public().to_string())
        );
        drop(browser);
    }
}
