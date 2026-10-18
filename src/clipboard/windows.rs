//! Windows backend: arboard for content, the clipboard sequence number for cheap change
//! detection, and the registered formats password managers use to opt out of syncing.

use super::ClipboardBackend;
use super::image_util::{png_to_rgba, rgba_to_png};
use crate::engine::Observed;
use crate::proto::Content;
use anyhow::Result;
use windows_sys::Win32::System::DataExchange::{
    CloseClipboard, GetClipboardData, GetClipboardSequenceNumber, IsClipboardFormatAvailable,
    OpenClipboard, RegisterClipboardFormatW,
};
use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};

pub struct WinClipboard {
    cb: arboard::Clipboard,
    last_seq: u32,
    our_seq: Option<u32>,
    /// Presence of any of these formats means "don't sync this".
    exclude_formats: [u32; 2],
    /// Holds a DWORD; 0 means "keep out of clipboard history / cloud clipboard".
    history_format: u32,
}

fn register(name: &str) -> u32 {
    let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
    // SAFETY: `wide` is a NUL-terminated UTF-16 string that outlives the call.
    unsafe { RegisterClipboardFormatW(wide.as_ptr()) }
}

fn sequence_number() -> u32 {
    // SAFETY: no preconditions.
    unsafe { GetClipboardSequenceNumber() }
}

impl WinClipboard {
    pub fn new() -> Result<Self> {
        Ok(Self {
            cb: arboard::Clipboard::new()?,
            last_seq: sequence_number(),
            our_seq: None,
            exclude_formats: [
                register("ExcludeClipboardContentFromMonitorProcessing"),
                register("Clipboard Viewer Ignore"),
            ],
            history_format: register("CanIncludeInClipboardHistory"),
        })
    }

    fn concealed(&self) -> bool {
        // SAFETY: plain Win32 clipboard calls; the global memory handle is only read while
        // the clipboard is open and the handle is locked.
        unsafe {
            let available = |f: u32| f != 0 && IsClipboardFormatAvailable(f) != 0;
            if self.exclude_formats.into_iter().any(available) {
                return true;
            }
            if !available(self.history_format) {
                return false;
            }
            if OpenClipboard(std::ptr::null_mut()) == 0 {
                // Couldn't check; err on the side of not sending.
                return true;
            }
            let mut value = None;
            let handle = GetClipboardData(self.history_format);
            if !handle.is_null() {
                let ptr = GlobalLock(handle) as *const u32;
                if !ptr.is_null() {
                    if GlobalSize(handle) >= 4 {
                        value = Some(ptr.read_unaligned());
                    }
                    GlobalUnlock(handle);
                }
            }
            CloseClipboard();
            value == Some(0)
        }
    }
}

impl ClipboardBackend for WinClipboard {
    fn name(&self) -> &'static str {
        "Windows clipboard"
    }

    fn poll(&mut self) -> Result<Option<Observed>> {
        let seq = sequence_number();
        if seq == self.last_seq {
            return Ok(None);
        }
        self.last_seq = seq;
        let mut obs = Observed {
            ours: self.our_seq == Some(seq),
            ..Default::default()
        };
        if obs.ours {
            return Ok(Some(obs));
        }
        obs.concealed = self.concealed();
        if !obs.concealed {
            obs.content = match self.cb.get_text() {
                Ok(text) if !text.is_empty() => Some(Content::Text(text)),
                _ => match self.cb.get_image() {
                    Ok(img) => Some(Content::Png(rgba_to_png(img.width, img.height, &img.bytes)?)),
                    Err(_) => None,
                },
            };
        }
        Ok(Some(obs))
    }

    fn write(&mut self, content: &Content) -> Result<()> {
        match content {
            Content::Text(s) => self.cb.set_text(s.clone())?,
            Content::Png(png) => {
                let (width, height, rgba) = png_to_rgba(png)?;
                self.cb.set_image(arboard::ImageData {
                    width,
                    height,
                    bytes: rgba.into(),
                })?;
            }
        }
        let seq = sequence_number();
        self.our_seq = Some(seq);
        self.last_seq = seq;
        Ok(())
    }
}
