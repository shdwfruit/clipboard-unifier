//! Sync decisions, independent of networking and of any real clipboard.

use crate::proto::{Clip, ClipId, Content, DeviceId};
use std::collections::{HashSet, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

/// What a clipboard backend saw when the local clipboard changed.
#[derive(Debug, Clone, Default)]
pub struct Observed {
    /// `None` when the new content is a type we don't sync (files, etc.).
    pub content: Option<Content>,
    /// Marked by a password manager as concealed/transient: never sent.
    pub concealed: bool,
    /// macOS: the content arrived from another Apple device via Universal Clipboard.
    pub via_universal_clipboard: bool,
    /// The backend recognised the change as one of its own writes.
    pub ours: bool,
}

#[derive(Debug)]
pub enum LocalDecision {
    Send(Clip),
    Skip(&'static str),
}

#[derive(Debug, PartialEq, Eq)]
pub enum RemoteDecision {
    Apply,
    Ignore(&'static str),
}

pub struct Engine {
    self_id: DeviceId,
    max_clip: usize,
    seen: SeenIds,
    /// Fingerprint of the clip we last wrote to the local clipboard. Local observations of
    /// the same content are echoes of that write, not new copies, until something else
    /// is copied.
    last_applied: Option<[u8; 32]>,
}

impl Engine {
    pub fn new(self_id: DeviceId, max_clip: usize) -> Self {
        Self {
            self_id,
            max_clip,
            seen: SeenIds::new(1024),
            last_applied: None,
        }
    }

    pub fn on_local(&mut self, obs: Observed) -> LocalDecision {
        if obs.ours {
            return LocalDecision::Skip("own write");
        }
        let fingerprint = obs.content.as_ref().map(Content::fingerprint);
        if fingerprint.is_some() && fingerprint == self.last_applied {
            return LocalDecision::Skip("echo of a received clip");
        }
        // Anything else is a genuine new local copy.
        self.last_applied = None;
        if obs.concealed {
            return LocalDecision::Skip("concealed (password manager)");
        }
        let Some(content) = obs.content else {
            return LocalDecision::Skip("unsupported clipboard type");
        };
        if content.is_empty() {
            return LocalDecision::Skip("empty");
        }
        if content.len() > self.max_clip {
            return LocalDecision::Skip("too large");
        }
        let clip = Clip {
            id: rand::random(),
            origin: self.self_id,
            created_ms: now_ms(),
            content,
        };
        self.seen.insert(clip.id);
        LocalDecision::Send(clip)
    }

    pub fn on_remote(&mut self, clip: &Clip) -> RemoteDecision {
        if clip.origin == self.self_id {
            return RemoteDecision::Ignore("our own clip");
        }
        if !self.seen.insert(clip.id) {
            return RemoteDecision::Ignore("already seen");
        }
        if clip.content.len() > self.max_clip {
            return RemoteDecision::Ignore("too large");
        }
        self.last_applied = Some(clip.content.fingerprint());
        RemoteDecision::Apply
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Bounded set of recently seen clip ids.
struct SeenIds {
    cap: usize,
    order: VecDeque<ClipId>,
    set: HashSet<ClipId>,
}

impl SeenIds {
    fn new(cap: usize) -> Self {
        Self {
            cap,
            order: VecDeque::new(),
            set: HashSet::new(),
        }
    }

    /// Returns false if the id was already present.
    fn insert(&mut self, id: ClipId) -> bool {
        if !self.set.insert(id) {
            return false;
        }
        self.order.push_back(id);
        if self.order.len() > self.cap
            && let Some(old) = self.order.pop_front()
        {
            self.set.remove(&old);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> Observed {
        Observed {
            content: Some(Content::Text(s.into())),
            ..Default::default()
        }
    }

    fn remote(origin: u8, s: &str) -> Clip {
        Clip {
            id: rand::random(),
            origin: [origin; 16],
            created_ms: 0,
            content: Content::Text(s.into()),
        }
    }

    #[test]
    fn local_copy_is_sent() {
        let mut e = Engine::new([1; 16], 100);
        let LocalDecision::Send(clip) = e.on_local(text("hi")) else {
            panic!()
        };
        assert_eq!(clip.origin, [1; 16]);
        assert_eq!(clip.content, Content::Text("hi".into()));
    }

    #[test]
    fn echo_of_applied_clip_is_suppressed() {
        let mut e = Engine::new([1; 16], 100);
        assert_eq!(e.on_remote(&remote(2, "x")), RemoteDecision::Apply);
        // The write we just made shows up as a local change (possibly more than once).
        assert!(matches!(e.on_local(text("x")), LocalDecision::Skip(_)));
        assert!(matches!(e.on_local(text("x")), LocalDecision::Skip(_)));
        // A different local copy is sent, and afterwards copying "x" again is a real copy.
        assert!(matches!(e.on_local(text("y")), LocalDecision::Send(_)));
        assert!(matches!(e.on_local(text("x")), LocalDecision::Send(_)));
    }

    #[test]
    fn duplicate_and_own_clips_are_ignored() {
        let mut e = Engine::new([1; 16], 100);
        let c = remote(2, "x");
        assert_eq!(e.on_remote(&c), RemoteDecision::Apply);
        assert!(matches!(e.on_remote(&c), RemoteDecision::Ignore(_)));
        let LocalDecision::Send(mine) = e.on_local(text("mine")) else {
            panic!()
        };
        assert!(matches!(e.on_remote(&mine), RemoteDecision::Ignore(_)));
    }

    #[test]
    fn concealed_and_unsupported_are_skipped() {
        let mut e = Engine::new([1; 16], 100);
        let mut secret = text("hunter2");
        secret.concealed = true;
        assert!(matches!(e.on_local(secret), LocalDecision::Skip(_)));
        assert!(matches!(
            e.on_local(Observed::default()),
            LocalDecision::Skip(_)
        ));
        let mut own = text("z");
        own.ours = true;
        assert!(matches!(e.on_local(own), LocalDecision::Skip(_)));
    }

    #[test]
    fn size_cap_applies_both_ways() {
        let mut e = Engine::new([1; 16], 4);
        assert!(matches!(e.on_local(text("12345")), LocalDecision::Skip(_)));
        assert!(matches!(e.on_local(text("1234")), LocalDecision::Send(_)));
        assert!(matches!(
            e.on_remote(&remote(2, "12345")),
            RemoteDecision::Ignore(_)
        ));
    }

    #[test]
    fn seen_ids_are_bounded() {
        let mut s = SeenIds::new(2);
        assert!(s.insert([1; 16]));
        assert!(s.insert([2; 16]));
        assert!(s.insert([3; 16]));
        assert!(s.insert([1; 16]), "oldest id was evicted");
        assert!(!s.insert([3; 16]));
    }
}
