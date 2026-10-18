//! Encrypted, authenticated channel: Noise NNpsk0 over a length-prefixed byte stream.
//!
//! Wire format: every frame is `u16 big-endian length || noise message` (≤ 65535 bytes).
//! Inside the channel, app messages are `u32 big-endian length || postcard(Message)`,
//! split across as many transport frames as needed.

use crate::proto::Message;
use anyhow::{Context, Result, bail};
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadHalf, WriteHalf};

const NOISE_PARAMS: &str = "Noise_NNpsk0_25519_ChaChaPoly_BLAKE2s";
const MAX_FRAME: usize = 65535;
const TAG_LEN: usize = 16;
const MAX_CHUNK: usize = MAX_FRAME - TAG_LEN;
/// Room for message framing on top of the clip payload.
const MESSAGE_OVERHEAD: usize = 64 * 1024;

type State = Arc<Mutex<snow::TransportState>>;

pub struct NoiseReader<S> {
    io: ReadHalf<S>,
    state: State,
    plain: Vec<u8>,
    frame: Vec<u8>,
    scratch: Vec<u8>,
    max_message: usize,
}

pub struct NoiseWriter<S> {
    io: WriteHalf<S>,
    state: State,
    out: Vec<u8>,
}

async fn read_frame<R: AsyncRead + Unpin>(r: &mut R, buf: &mut Vec<u8>) -> Result<()> {
    let len = r.read_u16().await? as usize;
    buf.resize(len, 0);
    r.read_exact(buf).await?;
    Ok(())
}

async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, data: &[u8]) -> Result<()> {
    let mut framed = Vec::with_capacity(2 + data.len());
    framed.extend_from_slice(&(data.len() as u16).to_be_bytes());
    framed.extend_from_slice(data);
    w.write_all(&framed).await?;
    w.flush().await?;
    Ok(())
}

/// Run the Noise handshake. Fails if the peer doesn't hold the same pre-shared key.
pub async fn handshake<S>(
    stream: S,
    psk: &[u8; 32],
    initiator: bool,
    max_clip: usize,
) -> Result<(NoiseReader<S>, NoiseWriter<S>)>
where
    S: AsyncRead + AsyncWrite + Send + 'static,
{
    let (mut r, mut w) = tokio::io::split(stream);
    let builder = snow::Builder::new(NOISE_PARAMS.parse()?).psk(0, psk)?;
    let mut hs = if initiator {
        builder.build_initiator()?
    } else {
        builder.build_responder()?
    };
    let mut buf = vec![0u8; MAX_FRAME];
    let mut frame = Vec::new();
    if initiator {
        let n = hs.write_message(&[], &mut buf)?;
        write_frame(&mut w, &buf[..n]).await?;
        read_frame(&mut r, &mut frame).await?;
        hs.read_message(&frame, &mut buf)
            .context("handshake rejected (different pairing code?)")?;
    } else {
        read_frame(&mut r, &mut frame).await?;
        hs.read_message(&frame, &mut buf)
            .context("handshake rejected (different pairing code?)")?;
        let n = hs.write_message(&[], &mut buf)?;
        write_frame(&mut w, &buf[..n]).await?;
    }
    let state = Arc::new(Mutex::new(hs.into_transport_mode()?));
    Ok((
        NoiseReader {
            io: r,
            state: state.clone(),
            plain: Vec::new(),
            frame: Vec::new(),
            scratch: vec![0u8; MAX_FRAME],
            max_message: max_clip + MESSAGE_OVERHEAD,
        },
        NoiseWriter {
            io: w,
            state,
            out: vec![0u8; MAX_FRAME],
        },
    ))
}

impl<S: AsyncRead> NoiseReader<S> {
    pub async fn recv(&mut self) -> Result<Message> {
        loop {
            if self.plain.len() >= 4 {
                let len = u32::from_be_bytes(self.plain[..4].try_into().unwrap()) as usize;
                if len > self.max_message {
                    bail!("peer sent an oversized message ({len} bytes)");
                }
                if self.plain.len() >= 4 + len {
                    let msg = postcard::from_bytes(&self.plain[4..4 + len])?;
                    self.plain.drain(..4 + len);
                    return Ok(msg);
                }
            }
            read_frame(&mut self.io, &mut self.frame).await?;
            let n = self
                .state
                .lock()
                .unwrap()
                .read_message(&self.frame, &mut self.scratch)?;
            self.plain.extend_from_slice(&self.scratch[..n]);
        }
    }
}

impl<S: AsyncWrite> NoiseWriter<S> {
    pub async fn send(&mut self, msg: &Message) -> Result<()> {
        let payload = postcard::to_stdvec(msg)?;
        let mut plain = Vec::with_capacity(4 + payload.len());
        plain.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        plain.extend_from_slice(&payload);
        for chunk in plain.chunks(MAX_CHUNK) {
            let n = self
                .state
                .lock()
                .unwrap()
                .write_message(chunk, &mut self.out)?;
            write_frame(&mut self.io, &self.out[..n]).await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Secret;
    use crate::proto::{Clip, Content};

    async fn pair(
        a: &Secret,
        b: &Secret,
    ) -> (
        Result<(NoiseReader<tokio::io::DuplexStream>, NoiseWriter<tokio::io::DuplexStream>)>,
        Result<(NoiseReader<tokio::io::DuplexStream>, NoiseWriter<tokio::io::DuplexStream>)>,
    ) {
        let (x, y) = tokio::io::duplex(1 << 20);
        let (pa, pb) = (a.psk(), b.psk());
        let ha = tokio::spawn(async move { handshake(x, &pa, true, 1 << 20).await });
        let hb = tokio::spawn(async move { handshake(y, &pb, false, 1 << 20).await });
        (ha.await.unwrap(), hb.await.unwrap())
    }

    #[tokio::test]
    async fn handshake_succeeds_with_same_secret() {
        let s = Secret::generate();
        let (a, b) = pair(&s, &s).await;
        let ((_ra, mut wa), (mut rb, _wb)) = (a.unwrap(), b.unwrap());
        wa.send(&Message::Ping).await.unwrap();
        assert!(matches!(rb.recv().await.unwrap(), Message::Ping));
    }

    #[tokio::test]
    async fn handshake_fails_with_different_secret() {
        let (a, b) = pair(&Secret::generate(), &Secret::generate()).await;
        // The responder rejects the first message; the initiator then sees the stream close.
        assert!(b.is_err());
        assert!(a.is_err());
    }

    #[tokio::test]
    async fn large_message_round_trips_across_frames() {
        let s = Secret::generate();
        let (a, b) = pair(&s, &s).await;
        let ((_ra, mut wa), (mut rb, _wb)) = (a.unwrap(), b.unwrap());
        let big: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect();
        let clip = Clip {
            id: [1; 16],
            origin: [2; 16],
            created_ms: 3,
            content: Content::Png(big.clone()),
        };
        let sender = tokio::spawn(async move {
            wa.send(&Message::Clip(clip)).await.unwrap();
            wa.send(&Message::Pong).await.unwrap();
            wa
        });
        match rb.recv().await.unwrap() {
            Message::Clip(c) => assert_eq!(c.content, Content::Png(big)),
            other => panic!("unexpected {other:?}"),
        }
        assert!(matches!(rb.recv().await.unwrap(), Message::Pong));
        sender.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_oversized_message() {
        let s = Secret::generate();
        let (x, y) = tokio::io::duplex(1 << 20);
        let psk = s.psk();
        let ha = tokio::spawn(async move { handshake(x, &psk, true, 1 << 20).await });
        let (mut rb, _wb) = handshake(y, &psk, false, 1000).await.unwrap();
        let (_ra, mut wa) = ha.await.unwrap().unwrap();
        tokio::spawn(async move {
            let _ = wa
                .send(&Message::Clip(Clip {
                    id: [0; 16],
                    origin: [0; 16],
                    created_ms: 0,
                    content: Content::Png(vec![0; 200_000]),
                }))
                .await;
        });
        assert!(rb.recv().await.is_err());
    }
}
