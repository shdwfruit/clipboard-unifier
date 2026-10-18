//! Platform clipboard access.
//!
//! Backends run on a dedicated thread (see `node`), are polled for changes, and are told
//! to write clips received from peers.

use crate::engine::Observed;
use crate::proto::Content;
use anyhow::Result;

#[cfg(not(target_os = "macos"))]
mod image_util;
#[cfg(not(any(windows, target_os = "macos")))]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
pub mod mock;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
pub use macos::REMOTE_MARKER_TYPES;

pub trait ClipboardBackend {
    fn name(&self) -> &'static str;

    /// Returns `Some` when the clipboard changed since the previous call.
    fn poll(&mut self) -> Result<Option<Observed>>;

    /// Replace the clipboard with content received from a peer.
    fn write(&mut self, content: &Content) -> Result<()>;

    /// Extra lines for `clipboard-unifier doctor`. Must not include clipboard contents.
    fn diagnostics(&mut self) -> Vec<String> {
        Vec::new()
    }

    /// Problems worth warning about when `run` starts (e.g. clipboard access denied).
    fn startup_warnings(&mut self) -> Vec<String> {
        Vec::new()
    }
}

/// Options that affect how received clips are written locally.
#[derive(Clone, Copy, Debug, Default)]
pub struct WriteOptions {
    /// macOS: keep bridged content on this Mac instead of offering it to the iPhone.
    pub local_only: bool,
}

#[cfg(target_os = "macos")]
pub fn platform_backend(opts: WriteOptions) -> Result<Box<dyn ClipboardBackend>> {
    Ok(Box::new(macos::MacClipboard::new(opts)))
}

#[cfg(windows)]
pub fn platform_backend(_opts: WriteOptions) -> Result<Box<dyn ClipboardBackend>> {
    Ok(Box::new(windows::WinClipboard::new()?))
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn platform_backend(_opts: WriteOptions) -> Result<Box<dyn ClipboardBackend>> {
    Ok(Box::new(linux::LinuxClipboard::new()?))
}
