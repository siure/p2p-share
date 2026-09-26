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
    encrypted_write_with_buffer(writer, transport, plaintext, &mut Vec::new()).await
}

/// Encrypt a frame using storage retained by the transfer loop.
pub async fn encrypted_write_with_buffer<W: AsyncWrite + Unpin>(
    writer: &mut W,
    transport: &mut TransportState,
    plaintext: &[u8],
    ciphertext: &mut Vec<u8>,
) -> Result<()> {
    if plaintext.len() > NOISE_MAX_MSG - 16 {
        bail!("plaintext frame too large");
    }
    ciphertext.resize(plaintext.len() + 16, 0);
    let len = transport.write_message(plaintext, ciphertext)?;
    send_frame(writer, &ciphertext[..len]).await
}

/// Read and decrypt one control frame.
pub async fn encrypted_read<R: AsyncRead + Unpin>(
    reader: &mut R,
    transport: &mut TransportState,
) -> Result<Vec<u8>> {
    let mut plaintext = Vec::new();
    encrypted_read_with_buffers(reader, transport, &mut Vec::new(), &mut plaintext).await?;
    Ok(plaintext)
}

/// Reuse both frame buffers; plaintext contains only the authenticated bytes.
pub async fn encrypted_read_with_buffers<R: AsyncRead + Unpin>(
    reader: &mut R,
    transport: &mut TransportState,
    ciphertext: &mut Vec<u8>,
    plaintext: &mut Vec<u8>,
) -> Result<usize> {
    recv_frame_into(reader, ciphertext).await?;
    plaintext.resize(ciphertext.len(), 0);
    let len = transport.read_message(ciphertext, plaintext)?;
    plaintext.truncate(len);
    Ok(len)
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
    let mut frame = Vec::new();
    recv_frame_into(reader, &mut frame).await?;
    Ok(frame)
}

async fn recv_frame_into<R: AsyncRead + Unpin>(reader: &mut R, frame: &mut Vec<u8>) -> Result<()> {
    let mut len_bytes = [0u8; 4];
    reader.read_exact(&mut len_bytes).await?;
    let len = u32::from_be_bytes(len_bytes) as usize;
    if len > NOISE_MAX_MSG {
        bail!("frame too large: {} bytes", len);
    }
    frame.resize(len, 0);
    reader.read_exact(frame).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reused_buffers_handle_different_lengths_and_reject_corruption() {
        let (mut left, mut right) = tokio::io::duplex(128 * 1024);
        let (mut left_read, mut left_write) = tokio::io::split(&mut left);
        let (mut right_read, mut right_write) = tokio::io::split(&mut right);
        let (initiator, responder) = tokio::join!(
            handshake_initiator(&mut left_read, &mut left_write),
            handshake_responder(&mut right_read, &mut right_write)
        );
        let (mut sender, _) = initiator.unwrap();
        let (mut receiver, _) = responder.unwrap();
        let mut send_buffer = Vec::new();
        let mut cipher = Vec::new();
        let mut plain = Vec::new();
        for payload in [vec![42; 60 * 1024], vec![7; 3], vec![9; 1024]] {
            encrypted_write_with_buffer(&mut left_write, &mut sender, &payload, &mut send_buffer)
                .await
                .unwrap();
            let len = encrypted_read_with_buffers(
                &mut right_read,
                &mut receiver,
                &mut cipher,
                &mut plain,
            )
            .await
            .unwrap();
            assert_eq!(len, payload.len());
            assert_eq!(plain, payload);
        }
        let mut bad = vec![0; 32];
        let len = sender.write_message(b"tamper", &mut bad).unwrap();
        bad[0] ^= 1;
        send_frame(&mut left_write, &bad[..len]).await.unwrap();
        assert!(encrypted_read_with_buffers(
            &mut right_read,
            &mut receiver,
            &mut cipher,
            &mut plain
        )
        .await
        .is_err());
    }
}
