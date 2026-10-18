//! Linux backend (X11 and Wayland via arboard). There is no cheap change counter, so we
//! poll and compare fingerprints.

use super::ClipboardBackend;
use super::image_util::{png_to_rgba, rgba_to_png};
use crate::engine::Observed;
use crate::proto::Content;
use anyhow::Result;
use sha2::{Digest, Sha256};

/// Images are only checked on every Nth poll, and only when the clipboard holds no text:
/// fetching and hashing a large screenshot several times a second would be wasteful.
const IMAGE_POLL_EVERY: u32 = 6;
const EMPTY: [u8; 32] = [0; 32];

pub struct LinuxClipboard {
    cb: arboard::Clipboard,
    last: Option<[u8; 32]>,
    ticks: u32,
}

fn text_fingerprint(s: &str) -> [u8; 32] {
    Sha256::new().chain_update(b"t").chain_update(s).finalize().into()
}

fn image_fingerprint(width: usize, height: usize, rgba: &[u8]) -> [u8; 32] {
    Sha256::new()
        .chain_update(b"i")
        .chain_update((width as u64).to_le_bytes())
        .chain_update((height as u64).to_le_bytes())
        .chain_update(rgba)
        .finalize()
        .into()
}

impl LinuxClipboard {
    pub fn new() -> Result<Self> {
        let mut me = Self {
            cb: arboard::Clipboard::new()?,
            last: None,
            ticks: 0,
        };
        // Don't broadcast whatever was on the clipboard before we started.
        me.ticks = IMAGE_POLL_EVERY - 1;
        let _ = me.poll();
        Ok(me)
    }
}

impl ClipboardBackend for LinuxClipboard {
    fn name(&self) -> &'static str {
        "Linux (arboard: X11 / Wayland data-control)"
    }

    fn poll(&mut self) -> Result<Option<Observed>> {
        self.ticks = self.ticks.wrapping_add(1);
        let (fingerprint, content) = match self.cb.get_text() {
            Ok(text) if !text.is_empty() => (text_fingerprint(&text), Some(Content::Text(text))),
            _ => {
                if !self.ticks.is_multiple_of(IMAGE_POLL_EVERY) {
                    return Ok(None);
                }
                match self.cb.get_image() {
                    Ok(img) => {
                        let fp = image_fingerprint(img.width, img.height, &img.bytes);
                        if Some(fp) == self.last {
                            return Ok(None);
                        }
                        let png = rgba_to_png(img.width, img.height, &img.bytes)?;
                        (fp, Some(Content::Png(png)))
                    }
                    Err(_) => (EMPTY, None),
                }
            }
        };
        if Some(fingerprint) == self.last {
            return Ok(None);
        }
        self.last = Some(fingerprint);
        Ok(Some(Observed {
            content,
            ..Default::default()
        }))
    }

    fn write(&mut self, content: &Content) -> Result<()> {
        // Record what we wrote so the next poll doesn't report it as a new copy.
        self.last = Some(match content {
            Content::Text(s) => {
                self.cb.set_text(s.clone())?;
                text_fingerprint(s)
            }
            Content::Png(png) => {
                let (width, height, rgba) = png_to_rgba(png)?;
                let fp = image_fingerprint(width, height, &rgba);
                self.cb.set_image(arboard::ImageData {
                    width,
                    height,
                    bytes: rgba.into(),
                })?;
                fp
            }
        });
        Ok(())
    }
}
