//! Multi-node tests on 127.0.0.1 with in-memory clipboards (no mDNS).

use clipbridge::clipboard::ClipboardBackend;
use clipbridge::clipboard::mock::MockClipboard;
use clipbridge::crypto::Secret;
use clipbridge::node::{Node, NodeConfig};
use clipbridge::proto::Content;
use std::time::Duration;

struct TestNode {
    node: Node,
    clip: MockClipboard,
}

impl TestNode {
    fn peer_addr(&self) -> String {
        self.node.local_addr.to_string()
    }
}

async fn start(secret: &Secret, peers: Vec<String>) -> TestNode {
    let clip = MockClipboard::default();
    let backend = clip.clone();
    let node = Node::start(
        NodeConfig {
            secret: secret.clone(),
            device_id: rand::random(),
            name: "test".into(),
            listen: "127.0.0.1:0".parse().unwrap(),
            static_peers: peers,
            discovery: false,
            max_clip: 1024 * 1024,
            poll_interval: Duration::from_millis(10),
        },
        Box::new(move || Ok(Box::new(backend) as Box<dyn ClipboardBackend>)),
    )
    .await
    .unwrap();
    TestNode { node, clip }
}

fn text(s: &str) -> Content {
    Content::Text(s.into())
}

async fn wait_for(clip: &MockClipboard, want: &Content) {
    for _ in 0..500 {
        if clip.current().as_ref() == Some(want) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {want:?}; have {:?}", clip.current());
}

/// Give any (unwanted) extra traffic time to show up.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(400)).await;
}

#[tokio::test]
async fn two_nodes_sync_both_ways_without_ping_pong() {
    let secret = Secret::generate();
    let a = start(&secret, vec![]).await;
    let b = start(&secret, vec![a.peer_addr()]).await;
    settle().await; // let B connect

    a.clip.user_copy(text("hello from A"));
    wait_for(&b.clip, &text("hello from A")).await;

    b.clip.user_copy(text("hello from B"));
    wait_for(&a.clip, &text("hello from B")).await;

    let png = Content::Png(vec![0x89, b'P', b'N', b'G', 1, 2, 3]);
    a.clip.user_copy(png.clone());
    wait_for(&b.clip, &png).await;

    settle().await;
    // Each side was written exactly once per clip from the other side: no echoes.
    assert_eq!(a.clip.writes(), vec![text("hello from B")]);
    assert_eq!(b.clip.writes(), vec![text("hello from A"), png]);
}

#[tokio::test]
async fn relays_through_a_middle_node() {
    let secret = Secret::generate();
    let hub = start(&secret, vec![]).await;
    let left = start(&secret, vec![hub.peer_addr()]).await;
    let right = start(&secret, vec![hub.peer_addr()]).await;
    settle().await;

    left.clip.user_copy(text("left → right"));
    wait_for(&right.clip, &text("left → right")).await;
    wait_for(&hub.clip, &text("left → right")).await;

    settle().await;
    assert!(left.clip.writes().is_empty(), "origin must not get its own clip back");
    assert_eq!(right.clip.writes().len(), 1);
    assert_eq!(hub.clip.writes().len(), 1);
}

#[tokio::test]
async fn wrong_pairing_code_cannot_sync() {
    let a = start(&Secret::generate(), vec![]).await;
    let intruder = start(&Secret::generate(), vec![a.peer_addr()]).await;
    settle().await;

    intruder.clip.user_copy(text("let me in"));
    a.clip.user_copy(text("secret stuff"));
    settle().await;
    assert!(a.clip.writes().is_empty());
    assert!(intruder.clip.writes().is_empty());
}

#[tokio::test]
async fn concealed_and_oversized_clips_stay_local() {
    let secret = Secret::generate();
    let a = start(&secret, vec![]).await;
    let b = start(&secret, vec![a.peer_addr()]).await;
    settle().await;

    a.clip.user_copy_concealed(text("hunter2"));
    a.clip.user_copy(Content::Png(vec![0; 2 * 1024 * 1024]));
    settle().await;
    assert!(b.clip.writes().is_empty());

    // The link still works afterwards.
    a.clip.user_copy(text("ok"));
    wait_for(&b.clip, &text("ok")).await;
}

#[tokio::test]
async fn duplicate_connections_collapse_to_one() {
    let secret = Secret::generate();
    let a = start(&secret, vec![]).await;
    // B dials A twice. Both sides must agree on which connection to keep; if they didn't,
    // each would close the other's choice and the link would drop until a redial.
    let b = start(&secret, vec![a.peer_addr(), a.peer_addr()]).await;
    settle().await;

    for i in 0..3 {
        let from_b = text(&format!("b{i}"));
        b.clip.user_copy(from_b.clone());
        wait_within(&a.clip, &from_b, Duration::from_millis(300)).await;
        let from_a = text(&format!("a{i}"));
        a.clip.user_copy(from_a.clone());
        wait_within(&b.clip, &from_a, Duration::from_millis(300)).await;
    }
    settle().await;
    assert_eq!(a.clip.writes().len(), 3);
    assert_eq!(b.clip.writes().len(), 3);
}

async fn wait_within(clip: &MockClipboard, want: &Content, limit: Duration) {
    let deadline = tokio::time::Instant::now() + limit;
    while tokio::time::Instant::now() < deadline {
        if clip.current().as_ref() == Some(want) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("{want:?} did not arrive within {limit:?}");
}
