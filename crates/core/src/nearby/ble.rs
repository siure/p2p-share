//! Bluetooth LE transport through BlueZ. Receivers advertise the service UUID
//! and serve their beacon over GATT; browsers scan for the UUID and read it.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use bluer::{
    adv::{Advertisement, AdvertisementHandle},
    gatt::local::{
        Application, ApplicationHandle, Characteristic, CharacteristicRead, CharacteristicWrite,
        CharacteristicWriteMethod, ReqError, Service,
    },
    Adapter, AdapterEvent, Address, DiscoveryFilter, DiscoveryTransport, Uuid,
};
use futures_util::{FutureExt, StreamExt};

use super::{
    hotspot, lock, Beacon, Hotspot, NetworkGuard, SharedTable, Via, BLE_BEACON_UUID,
    BLE_HOTSPOT_UUID, BLE_JOIN_UUID, BLE_SERVICE_UUID,
};
use crate::engine::{status, SharedSink};
use crate::ticket;

fn uuid(value: &str) -> Uuid {
    value.parse().expect("valid UUID constant")
}

async fn adapter() -> Result<Adapter> {
    let session = bluer::Session::new().await?;
    let adapter = session.default_adapter().await?;
    if !adapter.is_powered().await? {
        bail!("Bluetooth is off");
    }
    Ok(adapter)
}

pub(super) struct Advertiser {
    adapter: Adapter,
    beacon: Arc<Mutex<Vec<u8>>>,
    /// Started with the first beacon, so scanners never read an empty value.
    advertisement: Arc<tokio::sync::OnceCell<AdvertisementHandle>>,
    _application: ApplicationHandle,
    /// Hotspot joined at a sender's request; left when the receiver stops.
    _joined: Arc<tokio::sync::Mutex<Option<hotspot::Joined>>>,
}

impl Advertiser {
    pub(super) async fn start() -> Result<Self> {
        let adapter = adapter().await?;
        let beacon = Arc::new(Mutex::new(Vec::new()));
        let joined = Arc::new(tokio::sync::Mutex::new(None));
        let read_beacon = beacon.clone();
        let join_target = joined.clone();
        let application = Application {
            services: vec![Service {
                uuid: uuid(BLE_SERVICE_UUID),
                primary: true,
                characteristics: vec![
                    Characteristic {
                        uuid: uuid(BLE_BEACON_UUID),
                        read: Some(CharacteristicRead {
                            read: true,
                            fun: Box::new(move |request| {
                                let value = read_beacon.lock().unwrap_or_else(|e| e.into_inner());
                                // Long values are read in MTU-sized pieces.
                                let start = usize::from(request.offset).min(value.len());
                                let piece = value[start..].to_vec();
                                async move { Ok(piece) }.boxed()
                            }),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    Characteristic {
                        // Linux cannot host a hotspot here; senders fall back to hosting.
                        uuid: uuid(BLE_HOTSPOT_UUID),
                        read: Some(CharacteristicRead {
                            read: true,
                            fun: Box::new(|_| async { Ok(Vec::new()) }.boxed()),
                            ..Default::default()
                        }),
                        write: Some(CharacteristicWrite {
                            write: true,
                            method: CharacteristicWriteMethod::Fun(Box::new(|_, _| {
                                async { Err(ReqError::NotSupported) }.boxed()
                            })),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    Characteristic {
                        uuid: uuid(BLE_JOIN_UUID),
                        write: Some(CharacteristicWrite {
                            write: true,
                            method: CharacteristicWriteMethod::Fun(Box::new(move |value, _| {
                                let joined = join_target.clone();
                                async move {
                                    let network: Hotspot = serde_json::from_slice(&value)
                                        .map_err(|_| ReqError::InvalidValueLength)?;
                                    // Join in the background; the GATT write must answer quickly.
                                    tokio::spawn(async move {
                                        if let Ok(network) =
                                            hotspot::join(&network.ssid, &network.passphrase).await
                                        {
                                            *joined.lock().await = Some(network);
                                        }
                                    });
                                    Ok(())
                                }
                                .boxed()
                            })),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };
        let application = adapter.serve_gatt_application(application).await?;
        Ok(Self {
            adapter,
            beacon,
            advertisement: Arc::new(tokio::sync::OnceCell::new()),
            _application: application,
            _joined: joined,
        })
    }

    pub(super) fn update(&self, beacon: &Beacon) {
        *self.beacon.lock().unwrap_or_else(|e| e.into_inner()) = beacon.to_json().into_bytes();
        if self.advertisement.initialized() {
            return;
        }
        let (adapter, advertisement) = (self.adapter.clone(), self.advertisement.clone());
        // The handle stops advertising when the last reference (ours) is dropped.
        tokio::spawn(async move {
            let _ = advertisement
                .get_or_try_init(|| {
                    adapter.advertise(Advertisement {
                        service_uuids: [uuid(BLE_SERVICE_UUID)].into(),
                        discoverable: Some(true),
                        ..Default::default()
                    })
                })
                .await;
        });
    }
}

/// Scan until aborted. Each new device advertising the service is read once;
/// it is reported lost when BlueZ forgets it.
pub(super) async fn browse(table: SharedTable) {
    let Ok(adapter) = adapter().await else {
        return;
    };
    let filter = DiscoveryFilter {
        uuids: [uuid(BLE_SERVICE_UUID)].into(),
        transport: DiscoveryTransport::Le,
        ..Default::default()
    };
    if adapter.set_discovery_filter(filter).await.is_err() {
        return;
    }
    let Ok(events) = adapter.discover_devices().await else {
        return;
    };
    let mut events = std::pin::pin!(events);
    let mut ids = std::collections::HashMap::new();
    while let Some(event) = events.next().await {
        match event {
            AdapterEvent::DeviceAdded(address) => {
                if !advertises_service(&adapter, address).await {
                    continue;
                }
                if let Some(beacon) = read_beacon_retrying(&adapter, address).await {
                    if let Ok(id) = ticket::peer_id(&beacon.ticket) {
                        ids.insert(address, id);
                        let via = Via::Bluetooth(address.to_string());
                        lock(&table).seen(via, beacon.name, beacon.ticket);
                    }
                }
            }
            AdapterEvent::DeviceRemoved(address) => {
                if let Some(id) = ids.remove(&address) {
                    lock(&table).gone(&id, false);
                }
            }
            _ => {}
        }
    }
}

/// BlueZ also replays cached devices; only act on ones heard right now.
async fn advertises_service(adapter: &Adapter, address: Address) -> bool {
    let Ok(device) = adapter.device(address) else {
        return false;
    };
    let heard = device.rssi().await.ok().flatten().is_some();
    let uuids = device.uuids().await.ok().flatten().unwrap_or_default();
    heard && uuids.contains(&uuid(BLE_SERVICE_UUID))
}

/// Connect, run `action` on one characteristic of our service, disconnect.
async fn with_characteristic<T>(
    adapter: &Adapter,
    address: Address,
    characteristic: &str,
    action: impl AsyncFnOnce(bluer::gatt::remote::Characteristic) -> Result<T>,
) -> Result<T> {
    let device = adapter.device(address)?;
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        if !device.is_connected().await? {
            device.connect().await?;
        }
        for service in device.services().await? {
            if service.uuid().await? != uuid(BLE_SERVICE_UUID) {
                continue;
            }
            for found in service.characteristics().await? {
                if found.uuid().await? == uuid(characteristic) {
                    return action(found).await;
                }
            }
        }
        bail!("device does not offer nearby transfers")
    })
    .await
    .context("Bluetooth request timed out");
    let _ = device.disconnect().await;
    result?
}

/// `DeviceAdded` fires once per device, so ride out transient GATT failures here.
async fn read_beacon_retrying(adapter: &Adapter, address: Address) -> Option<Beacon> {
    for _ in 0..3 {
        if let Ok(beacon) = read_beacon(adapter, address).await {
            return Some(beacon);
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    None
}

async fn read_beacon(adapter: &Adapter, address: Address) -> Result<Beacon> {
    with_characteristic(adapter, address, BLE_BEACON_UUID, async |c| {
        Beacon::from_json(&c.read().await?)
    })
    .await
}

/// Poll the hotspot characteristic until the receiver lists its network.
async fn read_hotspot(adapter: &Adapter, address: Address) -> Result<Hotspot> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let value = with_characteristic(adapter, address, BLE_HOTSPOT_UUID, async |c| {
            Ok(c.read().await?)
        })
        .await;
        if let Ok(network) = serde_json::from_slice::<Hotspot>(&value.unwrap_or_default()) {
            return Ok(network);
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("no hotspot after 20 seconds");
        }
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }
}

/// Poll the beacon until `ready` accepts it.
async fn wait_for_beacon(
    adapter: &Adapter,
    address: Address,
    within: Duration,
    ready: impl Fn(&Beacon) -> bool,
) -> Option<Beacon> {
    let deadline = tokio::time::Instant::now() + within;
    let mut last = None;
    while tokio::time::Instant::now() < deadline {
        if let Ok(beacon) = read_beacon(adapter, address).await {
            if ready(&beacon) {
                return Some(beacon);
            }
            last = Some(beacon);
        }
        tokio::time::sleep(Duration::from_millis(1500)).await;
    }
    last
}

pub(super) async fn reach(
    address: &str,
    own_relay: bool,
    sink: Option<&SharedSink>,
) -> Result<(String, NetworkGuard)> {
    let adapter = adapter().await?;
    let address: Address = address.parse()?;
    let beacon = read_beacon(&adapter, address).await?;
    if own_relay && ticket::has_relay(&beacon.ticket) {
        return Ok((beacon.ticket, None));
    }
    status(
        sink,
        format!(
            "No shared network with {}. Asking it to start a hotspot...",
            beacon.name
        ),
    );
    with_characteristic(&adapter, address, BLE_HOTSPOT_UUID, async |c| {
        Ok(c.write(b"1").await?)
    })
    .await
    .context("The other device cannot start a hotspot and this one cannot host one. Connect both to the same network.")?;
    let network = read_hotspot(&adapter, address)
        .await
        .context("The other device did not start its hotspot.")?;
    status(sink, format!("Joining hotspot \"{}\"...", network.ssid));
    let joined = hotspot::join(&network.ssid, &network.passphrase).await?;
    // The receiver's ticket gains the hotspot address once its endpoint sees it.
    let ticket = wait_for_beacon(&adapter, address, Duration::from_secs(15), |b| {
        b.ticket != beacon.ticket
    })
    .await
    .map_or(beacon.ticket, |b| b.ticket);
    Ok((ticket, Some(Box::new(joined))))
}
