//! A running clipboard-unifier node: clipboard thread + sync engine + listener + dialers.

use crate::clipboard::ClipboardBackend;
use crate::config::DEFAULT_PORT;
use crate::crypto::Secret;
use crate::discovery::{self, Found};
use crate::engine::{Engine, LocalDecision, Observed, RemoteDecision};
use crate::proto::{Clip, Content, DeviceId, Hello, Message, PROTOCOL_VERSION, short_id};
use crate::transport;
use anyhow::{Context, Result, anyhow, bail};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};
use tracing::{debug, info, warn};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const PING_EVERY: Duration = Duration::from_secs(15);
const IDLE_TIMEOUT: Duration = Duration::from_secs(45);
const MIN_BACKOFF: Duration = Duration::from_secs(1);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const PEER_QUEUE: usize = 64;

pub struct NodeConfig {
    pub secret: Secret,
    pub device_id: DeviceId,
    pub name: String,
    pub listen: SocketAddr,
    /// `host` or `host:port`.
    pub static_peers: Vec<String>,
    pub discovery: bool,
    pub max_clip: usize,
    pub poll_interval: Duration,
}

pub type BackendFactory = Box<dyn FnOnce() -> Result<Box<dyn ClipboardBackend>> + Send>;

pub struct Node {
    pub local_addr: SocketAddr,
    pub backend: &'static str,
    tasks: Vec<JoinHandle<()>>,
    _discovery: Option<discovery::Discovery>,
}

impl Drop for Node {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

enum Event {
    Local(Observed),
    Remote {
        from: DeviceId,
        clip: Clip,
    },
    PeerUp {
        id: DeviceId,
        name: String,
        conn_id: u64,
        rank: (bool, u64),
        tx: mpsc::Sender<Arc<Message>>,
        accept: oneshot::Sender<bool>,
    },
    PeerDown {
        id: DeviceId,
        conn_id: u64,
    },
}

struct Ctx {
    self_id: DeviceId,
    name: String,
    psk: [u8; 32],
    max_clip: usize,
    events: mpsc::Sender<Event>,
    connected: Mutex<HashSet<DeviceId>>,
    next_conn: AtomicU64,
}

impl Ctx {
    fn is_connected(&self, id: &DeviceId) -> bool {
        self.connected.lock().unwrap().contains(id)
    }
}

impl Node {
    pub async fn start(cfg: NodeConfig, backend: BackendFactory) -> Result<Node> {
        let (events_tx, events_rx) = mpsc::channel(256);
        let (writes_tx, writes_rx) = std::sync::mpsc::channel();
        let backend_name =
            spawn_clipboard_thread(backend, cfg.poll_interval, events_tx.clone(), writes_rx)
                .await?;

        let listener = TcpListener::bind(cfg.listen)
            .await
            .with_context(|| format!("listening on {}", cfg.listen))?;
        let local_addr = listener.local_addr()?;

        let ctx = Arc::new(Ctx {
            self_id: cfg.device_id,
            name: cfg.name.clone(),
            psk: cfg.secret.psk(),
            max_clip: cfg.max_clip,
            events: events_tx,
            connected: Mutex::default(),
            next_conn: AtomicU64::new(1),
        });

        let mut tasks = vec![
            tokio::spawn(run_engine(
                Engine::new(cfg.device_id, cfg.max_clip),
                events_rx,
                writes_tx,
                ctx.clone(),
            )),
            tokio::spawn(accept_loop(listener, ctx.clone())),
        ];
        for peer in &cfg.static_peers {
            let target = Target::Host(with_default_port(peer));
            tasks.push(tokio::spawn(dial_loop(target, None, ctx.clone())));
        }

        let discovery = if cfg.discovery {
            let (d, found) = discovery::start(
                cfg.device_id,
                &cfg.name,
                local_addr.port(),
                cfg.secret.group_id(),
            )?;
            tasks.push(tokio::spawn(follow_discovery(found, ctx.clone())));
            Some(d)
        } else {
            None
        };

        Ok(Node {
            local_addr,
            backend: backend_name,
            tasks,
            _discovery: discovery,
        })
    }
}

fn with_default_port(peer: &str) -> String {
    let has_port = peer.parse::<SocketAddr>().is_ok()
        || peer
            .rsplit_once(':')
            .is_some_and(|(host, port)| !host.contains(':') && port.parse::<u16>().is_ok());
    if has_port {
        peer.to_string()
    } else {
        format!("{peer}:{DEFAULT_PORT}")
    }
}

/// Clipboard APIs are blocking (and on macOS, thread-affine), so they get their own thread.
async fn spawn_clipboard_thread(
    factory: BackendFactory,
    poll_interval: Duration,
    events: mpsc::Sender<Event>,
    writes: std::sync::mpsc::Receiver<Content>,
) -> Result<&'static str> {
    let (ready_tx, ready_rx) = oneshot::channel();
    std::thread::Builder::new()
        .name("clipboard".into())
        .spawn(move || {
            let mut backend = match factory() {
                Ok(mut b) => {
                    for w in b.startup_warnings() {
                        warn!("{w}");
                    }
                    let _ = ready_tx.send(Ok(b.name()));
                    b
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            use std::sync::mpsc::RecvTimeoutError;
            loop {
                match writes.recv_timeout(poll_interval) {
                    Ok(content) => {
                        if let Err(e) = backend.write(&content) {
                            warn!(
                                "writing {} to the clipboard failed: {e:#}",
                                content.describe()
                            );
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => match backend.poll() {
                        Ok(Some(obs)) => {
                            if events.blocking_send(Event::Local(obs)).is_err() {
                                break;
                            }
                        }
                        Ok(None) => {}
                        Err(e) => debug!("clipboard poll failed: {e:#}"),
                    },
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        })?;
    ready_rx
        .await
        .map_err(|_| anyhow!("clipboard thread exited during startup"))?
        .context("opening the clipboard")
}

struct Peer {
    name: String,
    conn_id: u64,
    rank: (bool, u64),
    tx: mpsc::Sender<Arc<Message>>,
}

fn broadcast(
    peers: &HashMap<DeviceId, Peer>,
    msg: &Arc<Message>,
    except: Option<DeviceId>,
) -> usize {
    let mut sent = 0;
    for (id, peer) in peers {
        if Some(*id) == except {
            continue;
        }
        match peer.tx.try_send(msg.clone()) {
            Ok(()) => sent += 1,
            Err(_) => warn!("{} is not keeping up; dropped a clip for it", peer.name),
        }
    }
    sent
}

async fn run_engine(
    mut engine: Engine,
    mut events: mpsc::Receiver<Event>,
    writes: std::sync::mpsc::Sender<Content>,
    ctx: Arc<Ctx>,
) {
    let mut peers: HashMap<DeviceId, Peer> = HashMap::new();
    while let Some(event) = events.recv().await {
        match event {
            Event::Local(obs) => {
                let via_uc = obs.via_universal_clipboard;
                match engine.on_local(obs) {
                    LocalDecision::Send(clip) => {
                        let what = clip.content.describe();
                        let n = broadcast(&peers, &Arc::new(Message::Clip(clip)), None);
                        let source = if via_uc {
                            "local copy (arrived via Universal Clipboard)"
                        } else {
                            "local copy"
                        };
                        info!("{source}: {what} → {n} peer(s)");
                    }
                    LocalDecision::Skip(reason) => {
                        debug!("local clipboard change not sent: {reason}");
                    }
                }
            }
            Event::Remote { from, clip } => {
                let sender = peers
                    .get(&from)
                    .map_or_else(|| short_id(&from), |p| p.name.clone());
                match engine.on_remote(&clip) {
                    RemoteDecision::Apply => {
                        let what = clip.content.describe();
                        if writes.send(clip.content.clone()).is_err() {
                            warn!("clipboard thread is gone");
                            return;
                        }
                        let n = broadcast(&peers, &Arc::new(Message::Clip(clip)), Some(from));
                        let fwd = if n > 0 {
                            format!(", forwarded to {n} peer(s)")
                        } else {
                            String::new()
                        };
                        info!("received {what} from {sender} → clipboard{fwd}");
                    }
                    RemoteDecision::Ignore(reason) => {
                        debug!("clip from {sender} ignored: {reason}");
                    }
                }
            }
            Event::PeerUp {
                id,
                name,
                conn_id,
                rank,
                tx,
                accept,
            } => {
                // Two connections to the same device (both sides dialed): keep the one both
                // sides rank best, so they agree on which to close.
                let keep = peers.get(&id).is_none_or(|existing| rank < existing.rank);
                if keep {
                    peers.insert(
                        id,
                        Peer {
                            name,
                            conn_id,
                            rank,
                            tx,
                        },
                    );
                    ctx.connected.lock().unwrap().insert(id);
                }
                let _ = accept.send(keep);
            }
            Event::PeerDown { id, conn_id } => {
                if peers.get(&id).is_some_and(|p| p.conn_id == conn_id) {
                    peers.remove(&id);
                    ctx.connected.lock().unwrap().remove(&id);
                }
            }
        }
    }
}

async fn accept_loop(listener: TcpListener, ctx: Arc<Ctx>) {
    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    if let Err(e) = run_connection(stream, false, ctx).await {
                        warn!("connection from {addr}: {e:#}");
                    }
                });
            }
            Err(e) => {
                warn!("accept failed: {e}");
                sleep(MIN_BACKOFF).await;
            }
        }
    }
}

/// Handshake, register with the engine, then pump messages until either side goes away.
/// Returns the peer's device id once the handshake succeeded.
async fn run_connection(stream: TcpStream, outbound: bool, ctx: Arc<Ctx>) -> Result<DeviceId> {
    stream.set_nodelay(true)?;
    let addr = stream.peer_addr()?;
    let (mut reader, mut writer) = timeout(
        HANDSHAKE_TIMEOUT,
        transport::handshake(stream, &ctx.psk, outbound, ctx.max_clip),
    )
    .await
    .context("handshake timed out")??;

    let my_nonce: u64 = rand::random();
    writer
        .send(&Message::Hello(Hello {
            protocol: PROTOCOL_VERSION,
            device_id: ctx.self_id,
            name: ctx.name.clone(),
            platform: std::env::consts::OS.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            conn_nonce: my_nonce,
        }))
        .await?;
    let hello = match timeout(HANDSHAKE_TIMEOUT, reader.recv())
        .await
        .context("timed out waiting for hello")??
    {
        Message::Hello(h) => h,
        _ => bail!("peer did not say hello"),
    };
    if hello.protocol != PROTOCOL_VERSION {
        bail!(
            "{} speaks protocol v{}; we speak v{PROTOCOL_VERSION}",
            hello.name,
            hello.protocol
        );
    }
    let peer_id = hello.device_id;
    if peer_id == ctx.self_id {
        bail!("connected to ourselves");
    }
    let (dialer, dialer_nonce) = if outbound {
        (ctx.self_id, my_nonce)
    } else {
        (peer_id, hello.conn_nonce)
    };
    let rank = (dialer != ctx.self_id.min(peer_id), dialer_nonce);
    let conn_id = ctx.next_conn.fetch_add(1, Ordering::Relaxed);

    let (out_tx, mut out_rx) = mpsc::channel::<Arc<Message>>(PEER_QUEUE);
    let (accept_tx, accept_rx) = oneshot::channel();
    ctx.events
        .send(Event::PeerUp {
            id: peer_id,
            name: hello.name.clone(),
            conn_id,
            rank,
            tx: out_tx,
            accept: accept_tx,
        })
        .await
        .map_err(|_| anyhow!("node is shutting down"))?;
    if !accept_rx.await.unwrap_or(false) {
        debug!("dropping duplicate connection to {}", hello.name);
        return Ok(peer_id);
    }
    info!(
        "connected to {} ({}, {addr}, {})",
        hello.name,
        hello.platform,
        if outbound { "outgoing" } else { "incoming" }
    );

    // Reads aren't cancel-safe, so they run in their own task.
    let (in_tx, mut in_rx) = mpsc::channel::<Message>(PEER_QUEUE);
    let read_task = tokio::spawn(async move {
        loop {
            match reader.recv().await {
                Ok(msg) => {
                    if in_tx.send(msg).await.is_err() {
                        break;
                    }
                }
                Err(e) => {
                    debug!("read ended: {e:#}");
                    break;
                }
            }
        }
    });

    let result: Result<()> = async {
        let mut ping = tokio::time::interval(PING_EVERY);
        ping.tick().await;
        let mut last_rx = Instant::now();
        loop {
            tokio::select! {
                msg = in_rx.recv() => {
                    let Some(msg) = msg else { return Ok(()) };
                    last_rx = Instant::now();
                    match msg {
                        Message::Ping => writer.send(&Message::Pong).await?,
                        Message::Clip(clip) => {
                            let _ = ctx.events.send(Event::Remote { from: peer_id, clip }).await;
                        }
                        Message::Pong | Message::Hello(_) => {}
                    }
                }
                msg = out_rx.recv() => {
                    // `None`: the engine dropped this connection (replaced by a duplicate).
                    let Some(msg) = msg else { return Ok(()) };
                    writer.send(&msg).await?;
                }
                _ = ping.tick() => {
                    if last_rx.elapsed() > IDLE_TIMEOUT {
                        bail!("no response for {}s", IDLE_TIMEOUT.as_secs());
                    }
                    writer.send(&Message::Ping).await?;
                }
            }
        }
    }
    .await;

    read_task.abort();
    let _ = ctx
        .events
        .send(Event::PeerDown {
            id: peer_id,
            conn_id,
        })
        .await;
    match result {
        Ok(()) => info!("disconnected from {}", hello.name),
        Err(e) => info!("disconnected from {}: {e:#}", hello.name),
    }
    Ok(peer_id)
}

enum Target {
    Host(String),
    Addrs(Vec<SocketAddr>),
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Target::Host(h) => f.write_str(h),
            Target::Addrs(a) => match a.first() {
                Some(first) => write!(f, "{first}"),
                None => f.write_str("(no address)"),
            },
        }
    }
}

async fn connect(target: &Target) -> Result<TcpStream> {
    match target {
        Target::Host(host) => {
            Ok(timeout(CONNECT_TIMEOUT, TcpStream::connect(host.as_str())).await??)
        }
        Target::Addrs(addrs) => {
            let mut last_err = anyhow!("no addresses");
            for addr in addrs {
                match timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
                    Ok(Ok(s)) => return Ok(s),
                    Ok(Err(e)) => last_err = e.into(),
                    Err(e) => last_err = e.into(),
                }
            }
            Err(last_err)
        }
    }
}

/// Keep a connection to `target` up, reconnecting with backoff.
async fn dial_loop(target: Target, mut known: Option<DeviceId>, ctx: Arc<Ctx>) {
    let mut backoff = MIN_BACKOFF;
    let mut failures = 0u32;
    loop {
        if known.is_some_and(|id| ctx.is_connected(&id)) {
            // Already connected to this device some other way (e.g. it dialed us).
            sleep(Duration::from_secs(3)).await;
            continue;
        }
        let started = Instant::now();
        match connect(&target).await {
            Ok(stream) => {
                failures = 0;
                match run_connection(stream, true, ctx.clone()).await {
                    Ok(id) => known = Some(id),
                    Err(e) => warn!("{target}: {e:#}"),
                }
            }
            Err(e) => {
                failures += 1;
                if failures == 1 {
                    warn!("can't reach {target} ({e}); will keep retrying");
                } else {
                    debug!("can't reach {target}: {e}");
                }
            }
        }
        if started.elapsed() > MAX_BACKOFF {
            backoff = MIN_BACKOFF;
        }
        sleep(backoff).await;
        backoff = (backoff * 2).min(MAX_BACKOFF);
    }
}

async fn follow_discovery(mut found: mpsc::UnboundedReceiver<Found>, ctx: Arc<Ctx>) {
    let mut dialers: HashMap<String, (Vec<SocketAddr>, JoinHandle<()>)> = HashMap::new();
    while let Some(event) = found.recv().await {
        match event {
            Found::Peer {
                fullname,
                id,
                addrs,
            } => {
                // Only the side with the lower id dials, so discovery doesn't open two
                // connections between the same pair.
                if ctx.self_id > id || addrs.is_empty() {
                    continue;
                }
                if dialers.get(&fullname).is_some_and(|(a, _)| *a == addrs) {
                    continue;
                }
                debug!("discovered {fullname} at {addrs:?}");
                let task = tokio::spawn(dial_loop(
                    Target::Addrs(addrs.clone()),
                    Some(id),
                    ctx.clone(),
                ));
                if let Some((_, old)) = dialers.insert(fullname, (addrs, task)) {
                    old.abort();
                }
            }
            Found::Gone { fullname } => {
                if let Some((_, task)) = dialers.remove(&fullname) {
                    task.abort();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn default_port_is_added_only_when_missing() {
        use super::with_default_port as p;
        assert_eq!(p("mac.local"), "mac.local:47321");
        assert_eq!(p("10.0.0.2"), "10.0.0.2:47321");
        assert_eq!(p("10.0.0.2:9"), "10.0.0.2:9");
        assert_eq!(p("[::1]:9"), "[::1]:9");
    }
}
