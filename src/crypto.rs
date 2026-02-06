use anyhow::{bail, Context, Result};
use snow::{Builder, TransportState};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Noise protocol pattern: NN (no static keys, ephemeral-only).
/// Cipher: ChaChaPoly.  DH: 25519.  Hash: BLAKE2s.
const NOISE_PATTERN: &str = "Noise_NN_25519_ChaChaPoly_BLAKE2s";

/// Maximum Noise transport message (ciphertext) size.
const NOISE_MAX_MSG: usize = 65535;

// ─── Handshake ──────────────────────────────────────────────────────────────

/// Perform the Noise NN handshake as the **initiator** (the receiver/client).
/// Returns the transport state and a verification code.
pub async fn handshake_initiator<R, W>(
    reader: &mut R,
    writer: &mut W,
) -> Result<(TransportState, String)>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let builder = Builder::new(NOISE_PATTERN.parse()?);
    let mut handshake = builder.build_initiator()?;

    let mut buf = vec![0u8; NOISE_MAX_MSG];

    // -> e  (initiator sends ephemeral public key)
    let len = handshake.write_message(&[], &mut buf)?;
    send_frame(writer, &buf[..len]).await?;

    // <- e, ee  (responder replies)
    let frame = recv_frame(reader).await?;
    handshake.read_message(&frame, &mut buf)?;

    let hash = handshake.get_handshake_hash().to_vec();
    let transport = handshake
        .into_transport_mode()
        .context("failed to enter transport mode")?;

    Ok((transport, verification_code(&hash)))
}

/// Perform the Noise NN handshake as the **responder** (the sender/server).
/// Returns the transport state and a verification code.
pub async fn handshake_responder<R, W>(
    reader: &mut R,
    writer: &mut W,
) -> Result<(TransportState, String)>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let builder = Builder::new(NOISE_PATTERN.parse()?);
    let mut handshake = builder.build_responder()?;

    let mut buf = vec![0u8; NOISE_MAX_MSG];

    // -> e  (read initiator's ephemeral public key)
    let frame = recv_frame(reader).await?;
    handshake.read_message(&frame, &mut buf)?;

    // <- e, ee  (respond with our ephemeral key)
    let len = handshake.write_message(&[], &mut buf)?;
    send_frame(writer, &buf[..len]).await?;

    let hash = handshake.get_handshake_hash().to_vec();
    let transport = handshake
        .into_transport_mode()
        .context("failed to enter transport mode")?;

    Ok((transport, verification_code(&hash)))
}

// ─── Verification code ──────────────────────────────────────────────────────

/// Derive a short human-readable verification code from the handshake hash.
/// Format: `xxxx-xxxx` (8 hex chars from the first 4 bytes of the hash).
fn verification_code(handshake_hash: &[u8]) -> String {
    let a = hex::encode(&handshake_hash[..2]);
    let b = hex::encode(&handshake_hash[2..4]);
    format!("{}-{}", a, b)
}

/// Tiny hex encoder (avoids adding a `hex` crate dependency).
mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }
}

// ─── Encrypted framing ─────────────────────────────────────────────────────

/// Encrypt `plaintext` and send it as a length-prefixed frame.
/// Wire format: [4-byte big-endian length][ciphertext].
pub async fn encrypted_write<W: AsyncWrite + Unpin>(
    writer: &mut W,
    transport: &mut TransportState,
    plaintext: &[u8],
) -> Result<()> {
    let mut ciphertext = vec![0u8; plaintext.len() + 16]; // 16-byte AEAD tag
    let len = transport.write_message(plaintext, &mut ciphertext)?;
    send_frame(writer, &ciphertext[..len]).await
}

/// Read a length-prefixed encrypted frame, decrypt it, return the plaintext.
pub async fn encrypted_read<R: AsyncRead + Unpin>(
    reader: &mut R,
    transport: &mut TransportState,
) -> Result<Vec<u8>> {
    let ciphertext = recv_frame(reader).await?;
    let mut plaintext = vec![0u8; ciphertext.len()];
    let len = transport.read_message(&ciphertext, &mut plaintext)?;
    plaintext.truncate(len);
    Ok(plaintext)
}

// ─── Raw framing helpers ────────────────────────────────────────────────────

/// Send a length-prefixed frame: [4-byte BE len][data].
async fn send_frame<W: AsyncWrite + Unpin>(writer: &mut W, data: &[u8]) -> Result<()> {
    if data.len() > NOISE_MAX_MSG {
        bail!("frame too large: {} bytes", data.len());
    }
    let len_bytes = (data.len() as u32).to_be_bytes();
    writer.write_all(&len_bytes).await?;
    writer.write_all(data).await?;
    writer.flush().await?;
    Ok(())
}

/// Read a length-prefixed frame.
async fn recv_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Vec<u8>> {
    let mut len_bytes = [0u8; 4];
    reader.read_exact(&mut len_bytes).await?;
    let len = u32::from_be_bytes(len_bytes) as usize;
    if len > NOISE_MAX_MSG {
        bail!("frame too large: {} bytes", len);
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;
    Ok(buf)
}
