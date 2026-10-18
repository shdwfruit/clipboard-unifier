//! Wire messages exchanged between peers (inside the encrypted channel).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub type DeviceId = [u8; 16];
pub type ClipId = [u8; 16];

pub const PROTOCOL_VERSION: u32 = 1;

/// Clipboard payload. Images always travel as PNG.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
pub enum Content {
    Text(String),
    Png(#[serde(with = "serde_bytes")] Vec<u8>),
}

impl Content {
    pub fn kind(&self) -> &'static str {
        match self {
            Content::Text(_) => "text",
            Content::Png(_) => "image",
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Content::Text(s) => s.len(),
            Content::Png(b) => b.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn fingerprint(&self) -> [u8; 32] {
        let mut h = Sha256::new();
        match self {
            Content::Text(s) => {
                h.update(b"t");
                h.update(s.as_bytes());
            }
            Content::Png(b) => {
                h.update(b"p");
                h.update(b);
            }
        }
        h.finalize().into()
    }

    /// Short description for logs: kind and size, never the content itself.
    pub fn describe(&self) -> String {
        format!("{} {}", self.kind(), human_bytes(self.len()))
    }
}

// Deliberately does not print the payload.
impl std::fmt::Debug for Content {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Content({})", self.describe())
    }
}

pub fn human_bytes(n: usize) -> String {
    match n {
        n if n < 1024 => format!("{n}B"),
        n if n < 1024 * 1024 => format!("{:.1}KB", n as f64 / 1024.0),
        n => format!("{:.1}MB", n as f64 / (1024.0 * 1024.0)),
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Clip {
    pub id: ClipId,
    pub origin: DeviceId,
    pub created_ms: u64,
    pub content: Content,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Hello {
    pub protocol: u32,
    pub device_id: DeviceId,
    pub name: String,
    pub platform: String,
    pub version: String,
    /// Random value chosen per connection; used to break ties between duplicate connections.
    pub conn_nonce: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Message {
    Hello(Hello),
    Clip(Clip),
    Ping,
    Pong,
}

pub fn short_id(id: &DeviceId) -> String {
    hex::encode(&id[..4])
}
