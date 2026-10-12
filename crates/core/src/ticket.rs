use std::net::{IpAddr, SocketAddr};

use anyhow::{bail, Context, Result};
use iroh::{EndpointAddr, TransportAddr};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Ticket {
    transport_version: u8,
    endpoint: EndpointAddr,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session_alpn: Option<String>,
}

const TICKET_PREFIX: &str = "p2psh";

/// Serialize an `EndpointAddr` into a compact, copy-pasteable ticket string.
///
/// Format: `p2psh:<base64url-encoded JSON>`
///
/// Before encoding we strip out addresses that are almost certainly useless to
/// a remote peer (loopback and link-local) so the
/// resulting ticket is as short as possible — important for phone copy-paste.
pub fn serialize(addr: &EndpointAddr) -> Result<String> {
    serialize_session(addr, None)
}

pub(crate) fn serialize_session(addr: &EndpointAddr, session_alpn: Option<&str>) -> Result<String> {
    let filtered = filter_node_addr(addr);
    let json = serde_json::to_vec(&Ticket {
        transport_version: 1,
        endpoint: filtered,
        session_alpn: session_alpn.map(str::to_owned),
    })
    .context("failed to serialize node address")?;
    let encoded = data_encoding::BASE64URL_NOPAD.encode(&json);
    Ok(format!("{}:{}", TICKET_PREFIX, encoded))
}

/// Deserialize a ticket string back into an `EndpointAddr`.
pub fn deserialize(ticket: &str) -> Result<EndpointAddr> {
    Ok(decode(ticket)?.endpoint)
}

/// Returns the peer address and session ALPN when the ticket's session belongs
/// to one of the `accepted` protocol families.
pub(crate) fn connection_target(
    ticket: &str,
    accepted: &[&[u8]],
) -> Result<(EndpointAddr, Vec<u8>)> {
    let decoded = decode(ticket)?;
    let alpn = decoded
        .session_alpn
        .context("Ticket has no transfer session. Generate a new ticket on the other device.")?;
    if !accepted.iter().any(|family| session_matches(&alpn, family)) {
        bail!("Ticket is for a different transfer direction or has an invalid session. Generate a new ticket on the other device.");
    }
    Ok((decoded.endpoint, alpn.into_bytes()))
}

pub(crate) fn session_matches(alpn: &str, family: &[u8]) -> bool {
    let Ok(family) = std::str::from_utf8(family) else {
        return false;
    };
    alpn.strip_prefix(family)
        .and_then(|rest| rest.strip_prefix('/'))
        .is_some_and(|nonce| nonce.len() == 64 && nonce.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// Endpoint id of the ticket's peer, used to merge the same device seen over
/// several discovery transports.
pub fn peer_id(ticket: &str) -> Result<String> {
    Ok(decode(ticket)?.endpoint.id.to_string())
}

/// Whether the peer could be reached through a relay when the ticket was made.
pub fn has_relay(ticket: &str) -> bool {
    decode(ticket).is_ok_and(|t| t.endpoint.relay_urls().next().is_some())
}

/// Re-encode a ticket within `max_len` bytes by dropping IP addresses, IPv6
/// first. The relay URL is kept because it works across networks. Used for
/// Bluetooth beacons, whose GATT values are limited to 512 bytes.
pub fn compact(ticket: &str, max_len: usize) -> Result<String> {
    let decoded = decode(ticket)?;
    let session = decoded.session_alpn.as_deref();
    let mut addrs: Vec<TransportAddr> = filter_node_addr(&decoded.endpoint)
        .addrs
        .into_iter()
        .collect();
    // Truncation drops from the end, so order by likely usefulness: relays, then
    // home/office LAN ranges, then other private ranges (often container or VPN
    // bridges), then public IPv4, then IPv6.
    addrs.sort_by_key(|addr| match addr {
        TransportAddr::Relay(_) => 0,
        TransportAddr::Ip(SocketAddr::V4(ip)) => match ip.ip().octets() {
            [192, 168, ..] => 1,
            [10, ..] => 2,
            [172, b, ..] if (16..32).contains(&b) => 3,
            _ => 4,
        },
        _ => 5,
    });
    while !addrs.is_empty() {
        let endpoint = EndpointAddr::from_parts(decoded.endpoint.id, addrs.iter().cloned());
        let encoded = serialize_session(&endpoint, session)?;
        if encoded.len() <= max_len {
            return Ok(encoded);
        }
        addrs.pop();
    }
    bail!("ticket does not fit in {max_len} bytes")
}

fn decode(ticket: &str) -> Result<Ticket> {
    let (prefix, data) = ticket
        .trim()
        .split_once(':')
        .context("invalid ticket: expected 'p2psh:' prefix")?;
    if !prefix.eq_ignore_ascii_case(TICKET_PREFIX) {
        bail!("invalid ticket: expected 'p2psh:' prefix");
    }
    let bytes = data_encoding::BASE64URL_NOPAD
        .decode(data.as_bytes())
        .context(
            "invalid ticket: bad base64 encoding (was the ticket truncated during copy-paste?)",
        )?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).context("invalid ticket: corrupt address data")?;
    if value.get("transport_version").and_then(|v| v.as_u64()) != Some(1) {
        bail!("This ticket uses an incompatible network version. Update p2p-share on both devices and generate a new ticket.");
    }
    let ticket: Ticket =
        serde_json::from_value(value).context("invalid ticket: corrupt address data")?;
    if !has_usable_address(&ticket.endpoint) {
        bail!("invalid ticket: no usable addresses or relay URL");
    }

    Ok(ticket)
}

/// Check whether a string looks like an iroh ticket (vs. a plain ip:port address).
pub fn is_ticket(s: &str) -> bool {
    s.trim()
        .split_once(':')
        .is_some_and(|(prefix, _)| prefix.eq_ignore_ascii_case(TICKET_PREFIX))
}

/// Build a new `EndpointAddr` keeping only addresses that are useful to a remote
/// peer.  This drops loopback and link-local.
fn filter_node_addr(addr: &EndpointAddr) -> EndpointAddr {
    EndpointAddr::from_parts(
        addr.id,
        addr.addrs
            .iter()
            .filter(|address| match address {
                TransportAddr::Ip(ip) => is_useful_address(ip),
                TransportAddr::Relay(_) => true,
                _ => false,
            })
            .cloned(),
    )
}

pub(crate) fn has_usable_address(addr: &EndpointAddr) -> bool {
    !filter_node_addr(addr).addrs.is_empty()
}

/// Heuristic to decide whether a local address is worth advertising to a remote
/// peer in the ticket.
fn is_useful_address(addr: &SocketAddr) -> bool {
    match addr.ip() {
        IpAddr::V4(ip) => {
            // Loopback (127.x.x.x) — useless remotely.
            if ip.is_loopback() {
                return false;
            }
            // Link-local (169.254.x.x) — not routable.
            let o = ip.octets();
            if o[0] == 169 && o[1] == 254 {
                return false;
            }
            true
        }
        IpAddr::V6(ip) => {
            if ip.is_loopback() {
                return false;
            }
            // Link-local (fe80::/10)
            let seg = ip.segments();
            if seg[0] & 0xffc0 == 0xfe80 {
                return false;
            }
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address() -> EndpointAddr {
        EndpointAddr::new(iroh::SecretKey::generate().public())
            .with_ip_addr("172.16.0.1:9000".parse().unwrap())
    }

    #[test]
    fn current_ticket_roundtrips_private_lan_address_and_uppercase_prefix() {
        let addr = address();
        let encoded = serialize(&addr).unwrap();
        assert_eq!(deserialize(&encoded).unwrap(), addr);
        assert_eq!(
            deserialize(&encoded.replacen("p2psh:", "P2PSH:", 1)).unwrap(),
            addr
        );
    }

    #[test]
    fn legacy_ticket_requires_updating_both_devices() {
        let payload =
            data_encoding::BASE64URL_NOPAD.encode(br#"{"node_id":"old","direct_addresses":[]}"#);
        let error = deserialize(&format!("p2psh:{payload}"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("Update p2p-share on both devices"));
    }

    #[test]
    fn session_ticket_is_bound_to_transfer_direction() {
        let nonce = "a".repeat(64);
        let encoded = serialize_session(&address(), Some(&format!("p2p-share/2/{nonce}"))).unwrap();
        assert!(connection_target(&encoded, &[crate::engine::ALPN]).is_ok());
        assert!(connection_target(&encoded, &[crate::engine::ALPN_REVERSE]).is_err());
        assert!(connection_target(&encoded, &[crate::engine::ALPN_NEARBY]).is_err());
        assert!(
            connection_target(&serialize(&address()).unwrap(), &[crate::engine::ALPN]).is_err()
        );
        let nearby =
            serialize_session(&address(), Some(&format!("p2p-share/2-nearby/{nonce}"))).unwrap();
        let accepted = [crate::engine::ALPN_REVERSE, crate::engine::ALPN_NEARBY];
        assert!(connection_target(&nearby, &accepted).is_ok());
    }

    #[test]
    fn compact_ticket_keeps_relay_and_lan_address_over_bridges() {
        let mut addr = EndpointAddr::new(iroh::SecretKey::generate().public())
            .with_relay_url("https://relay.example./".parse().unwrap());
        for bridge in 17..30 {
            addr = addr.with_ip_addr(format!("172.{bridge}.0.1:4000").parse().unwrap());
        }
        addr = addr.with_ip_addr("192.168.1.20:4000".parse().unwrap());
        for i in 1..12 {
            addr = addr.with_ip_addr(format!("[2001:db8::{i}]:4000").parse().unwrap());
        }
        let session = format!("p2p-share/2-nearby/{}", "b".repeat(64));
        let full = serialize_session(&addr, Some(&session)).unwrap();
        let small = compact(&full, 480).unwrap();
        assert!(small.len() <= 480 && small.len() < full.len());
        let kept = deserialize(&small).unwrap();
        assert!(kept.relay_urls().next().is_some());
        assert!(kept
            .ip_addrs()
            .any(|ip| ip.to_string() == "192.168.1.20:4000"));
        assert_eq!(peer_id(&small).unwrap(), addr.id.to_string());
        assert!(has_relay(&small));
        assert!(compact(&full, 40).is_err());
    }

    #[test]
    fn ticket_prefix_detection_is_case_insensitive() {
        assert!(is_ticket("p2psh:abc"));
        assert!(is_ticket("P2PSH:abc"));
        assert!(is_ticket("P2pSh:abc"));
        assert!(!is_ticket("127.0.0.1:9000"));
    }
}
