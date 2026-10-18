//! Pairing secret, pairing codes, and keys derived from the secret.

use anyhow::{Result, bail};
use data_encoding::BASE32_NOPAD;
use hkdf::Hkdf;
use sha2::Sha256;

const CODE_PREFIX: &str = "cb1-";

/// The 32-byte group secret shared by every machine in a clipbridge group.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret([u8; 32]);

impl Secret {
    pub fn generate() -> Self {
        Self(rand::random())
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Human-shareable pairing code, e.g. `cb1-abcd-efgh-...`.
    pub fn to_code(&self) -> String {
        let b32 = BASE32_NOPAD.encode(&self.0).to_lowercase();
        let groups: Vec<&str> = b32
            .as_bytes()
            .chunks(4)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect();
        format!("{CODE_PREFIX}{}", groups.join("-"))
    }

    pub fn from_code(code: &str) -> Result<Self> {
        let code = code.trim();
        let Some(body) = code.strip_prefix(CODE_PREFIX) else {
            bail!("pairing code must start with `{CODE_PREFIX}`");
        };
        let cleaned: String = body
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '-')
            .collect::<String>()
            .to_uppercase();
        let bytes = BASE32_NOPAD
            .decode(cleaned.as_bytes())
            .map_err(|e| anyhow::anyhow!("invalid pairing code: {e}"))?;
        let Ok(bytes) = <[u8; 32]>::try_from(bytes) else {
            bail!("invalid pairing code: wrong length");
        };
        Ok(Self(bytes))
    }

    fn derive(&self, info: &[u8]) -> [u8; 32] {
        let mut out = [0u8; 32];
        Hkdf::<Sha256>::new(None, &self.0)
            .expand(info, &mut out)
            .expect("32 bytes is a valid HKDF-SHA256 output length");
        out
    }

    /// Pre-shared key for the Noise handshake.
    pub fn psk(&self) -> [u8; 32] {
        self.derive(b"clipbridge v1 psk")
    }

    /// Public group identifier advertised over mDNS (does not reveal the secret).
    pub fn group_id(&self) -> String {
        hex::encode(&self.derive(b"clipbridge v1 group")[..8])
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_round_trips() {
        let s = Secret::generate();
        let code = s.to_code();
        assert!(code.starts_with("cb1-"));
        assert_eq!(Secret::from_code(&code).unwrap(), s);
        // Case and separators don't matter.
        let sloppy = format!("  cb1-{}  ", code[4..].replace('-', "").to_uppercase());
        assert_eq!(Secret::from_code(&sloppy).unwrap(), s);
    }

    #[test]
    fn rejects_bad_codes() {
        assert!(Secret::from_code("hello").is_err());
        assert!(Secret::from_code("cb1-abcd").is_err());
    }

    #[test]
    fn derived_keys_differ() {
        let s = Secret::generate();
        assert_ne!(s.psk()[..8], hex::decode(s.group_id()).unwrap()[..]);
        assert_eq!(s.group_id().len(), 16);
    }
}
