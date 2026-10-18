//! In-memory clipboard for tests.

use super::ClipboardBackend;
use crate::engine::Observed;
use crate::proto::Content;
use anyhow::Result;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct State {
    current: Option<Content>,
    concealed: bool,
    generation: u64,
    polled: u64,
    writes: Vec<Content>,
}

/// Cloneable handle: the test keeps one clone, the node owns another.
///
/// Like the Linux backend, it does not flag its own writes, so the engine's echo
/// suppression is what keeps clips from bouncing back.
#[derive(Clone, Default)]
pub struct MockClipboard(Arc<Mutex<State>>);

impl MockClipboard {
    /// Simulate the user copying something on this machine.
    pub fn user_copy(&self, content: Content) {
        self.set(content, false);
    }

    /// Simulate a password manager copying a concealed value.
    pub fn user_copy_concealed(&self, content: Content) {
        self.set(content, true);
    }

    fn set(&self, content: Content, concealed: bool) {
        let mut s = self.0.lock().unwrap();
        s.current = Some(content);
        s.concealed = concealed;
        s.generation += 1;
    }

    pub fn current(&self) -> Option<Content> {
        self.0.lock().unwrap().current.clone()
    }

    /// Everything clipboard-unifier wrote to this clipboard, in order.
    pub fn writes(&self) -> Vec<Content> {
        self.0.lock().unwrap().writes.clone()
    }
}

impl ClipboardBackend for MockClipboard {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn poll(&mut self) -> Result<Option<Observed>> {
        let mut s = self.0.lock().unwrap();
        if s.generation == s.polled {
            return Ok(None);
        }
        s.polled = s.generation;
        Ok(Some(Observed {
            content: s.current.clone(),
            concealed: s.concealed,
            ..Default::default()
        }))
    }

    fn write(&mut self, content: &Content) -> Result<()> {
        let mut s = self.0.lock().unwrap();
        s.writes.push(content.clone());
        s.current = Some(content.clone());
        s.concealed = false;
        s.generation += 1;
        Ok(())
    }
}
