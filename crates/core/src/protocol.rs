use serde::{Deserialize, Serialize};

/// Size of each plaintext chunk before encryption: 60 KiB.
/// Kept under 65535 bytes (Noise max message) to leave room for the 16-byte
/// AEAD tag that Noise appends.
pub const CHUNK_SIZE: usize = 60 * 1024;

/// Header sent by the sender before the file data.
#[derive(Debug, Serialize, Deserialize)]
pub struct FileHeader {
    /// Original file name (just the name, no path components).
    pub name: String,
    /// Total size in bytes.
    pub size: u64,
    /// Hex-encoded blake3 hash of the file contents.
    pub blake3: String,
}

/// Single-line JSON terminated by `\n`, so the receiver can read it with
/// `read_line`.
impl FileHeader {
    pub fn to_wire(&self) -> anyhow::Result<Vec<u8>> {
        let mut buf = serde_json::to_vec(self)?;
        buf.push(b'\n');
        Ok(buf)
    }

    pub fn from_wire(line: &str) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(line.trim())?)
    }
}

/// Format bytes into a human-readable string (e.g. "1.23 MiB").
pub fn human_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;

    let b = bytes as f64;
    if b < KIB {
        format!("{} B", bytes)
    } else if b < MIB {
        format!("{:.2} KiB", b / KIB)
    } else if b < GIB {
        format!("{:.2} MiB", b / MIB)
    } else {
        format!("{:.2} GiB", b / GIB)
    }
}

#[cfg(test)]
mod tests {
    use super::{human_bytes, FileHeader};

    #[test]
    fn header_round_trip() {
        let header = FileHeader {
            name: "demo.txt".to_string(),
            size: 42,
            blake3: "abc123".to_string(),
        };
        let wire = header.to_wire().expect("serialize");
        let wire_str = String::from_utf8(wire).expect("utf8");
        let parsed = FileHeader::from_wire(&wire_str).expect("parse");
        assert_eq!(parsed.name, "demo.txt");
        assert_eq!(parsed.size, 42);
        assert_eq!(parsed.blake3, "abc123");
    }

    #[test]
    fn human_bytes_formats_expected_units() {
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(2048), "2.00 KiB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.00 MiB");
    }
}
