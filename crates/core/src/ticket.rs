use std::net::{IpAddr, SocketAddr};

use anyhow::{bail, Context, Result};
use iroh::NodeAddr;

const TICKET_PREFIX: &str = "p2psh";

/// Serialize a `NodeAddr` into a compact, copy-pasteable ticket string.
///
/// Format: `p2psh:<base64url-encoded JSON>`
///
/// Before encoding we strip out addresses that are almost certainly useless to
/// a remote peer (Docker/container bridges, loopback, link-local) so the
/// resulting ticket is as short as possible — important for phone copy-paste.
pub fn serialize(addr: &NodeAddr) -> Result<String> {
    let filtered = filter_node_addr(addr);
    let json = serde_json::to_vec(&filtered).context("failed to serialize node address")?;
    let encoded = data_encoding::BASE64URL_NOPAD.encode(&json);
    Ok(format!("{}:{}", TICKET_PREFIX, encoded))
}

/// Deserialize a ticket string back into a `NodeAddr`.
pub fn deserialize(ticket: &str) -> Result<NodeAddr> {
    let data = ticket
        .strip_prefix(&format!("{}:", TICKET_PREFIX))
        .context("invalid ticket: expected 'p2psh:' prefix")?;
    let bytes = data_encoding::BASE64URL_NOPAD
        .decode(data.as_bytes())
        .context(
            "invalid ticket: bad base64 encoding (was the ticket truncated during copy-paste?)",
        )?;
    let addr: NodeAddr = serde_json::from_slice(&bytes).context(
        "invalid ticket: corrupt address data (was the ticket truncated during copy-paste?)",
    )?;

    if addr.direct_addresses.is_empty() && addr.relay_url.is_none() {
        bail!("invalid ticket: no addresses or relay URL (was the ticket truncated during copy-paste?)");
    }

    Ok(addr)
}

/// Check whether a string looks like an iroh ticket (vs. a plain ip:port address).
pub fn is_ticket(s: &str) -> bool {
    let s = s.trim();
    s.starts_with(&format!("{}:", TICKET_PREFIX))
        || s.starts_with(&format!("{}:", TICKET_PREFIX.to_uppercase()))
}

/// Build a new `NodeAddr` keeping only addresses that are useful to a remote
/// peer.  This drops Docker/container bridges, loopback, and link-local.
fn filter_node_addr(addr: &NodeAddr) -> NodeAddr {
    let useful: Vec<SocketAddr> = addr
        .direct_addresses
        .iter()
        .copied()
        .filter(|sa| is_useful_address(sa))
        .collect();

    NodeAddr::from_parts(addr.node_id, addr.relay_url.clone(), useful)
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
            // Docker / container bridge gateways: 172.16-31.x.1 where
            // the third octet is 0 and fourth is 1.  Real LAN networks on
            // 172.16/12 almost never use x.0.1 as a host address.
            if o[0] == 172 && (16..=31).contains(&o[1]) && o[2] == 0 && o[3] == 1 {
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
    use super::is_ticket;

    #[test]
    fn ticket_prefix_detection_is_case_insensitive() {
        assert!(is_ticket("p2psh:abc"));
        assert!(is_ticket("P2PSH:abc"));
        assert!(!is_ticket("127.0.0.1:9000"));
    }
}
