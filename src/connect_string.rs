use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};

const SCHEME: &str = "p2p-share://v1/";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectMeta {
    pub name: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectBundle {
    pub ver: u8,
    pub salt: String, // base64url(no pad)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meta: Option<ConnectMeta>,
    // Fields below reserved for Phase 2+:
    // #[serde(skip_serializing_if = "Option::is_none")]
    // pub pid: Option<String>,
    // #[serde(skip_serializing_if = "Option::is_none")]
    // pub addrs: Option<Vec<String>>,
}

pub fn build_connect_string(
    meta: Option<ConnectMeta>,
    salt: &[u8; 16],
) -> Result<String> {
    let bundle = ConnectBundle {
        ver: 1,
        salt: URL_SAFE_NO_PAD.encode(salt),
        meta,
    };
    let json = serde_json::to_vec(&bundle)?;
    let b64 = URL_SAFE_NO_PAD.encode(json);
    Ok(format!("{}{}", SCHEME, b64))
}

pub fn parse_connect_string(s: &str) -> Result<ConnectBundle> {
    if !s.starts_with(SCHEME) {
        bail!("Invalid connect string: missing '{}'", SCHEME);
    }
    let b64 = &s[SCHEME.len()..];
    let raw = URL_SAFE_NO_PAD
        .decode(b64)
        .with_context(|| "Base64url decode failed")?;
    let bundle: ConnectBundle = serde_json::from_slice(&raw)
        .with_context(|| "JSON parse failed")?;
    if bundle.ver != 1 {
        bail!("Unsupported connect string version {}", bundle.ver);
    }
    Ok(bundle)
}