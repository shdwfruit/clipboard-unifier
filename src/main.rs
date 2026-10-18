use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use clipbridge::clipboard::{self, WriteOptions};
use clipbridge::config::{self, Config, DEFAULT_PORT};
use clipbridge::crypto::Secret;
use clipbridge::node::{Node, NodeConfig};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::info;

#[derive(Parser)]
#[command(
    name = "clipbridge",
    version,
    about = "Share your clipboard between Mac, Windows and Linux — and, through the Mac's \
             Universal Clipboard, your iPhone."
)]
struct Cli {
    /// Config file (default: platform config dir/clipbridge/config.toml).
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a new group on this machine and print its pairing code.
    Init {
        /// Name shown to other machines (default: hostname).
        #[arg(long)]
        name: Option<String>,
        /// Replace an existing config.
        #[arg(long)]
        force: bool,
    },
    /// Join an existing group using a pairing code from `clipbridge init`.
    Join {
        code: String,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        force: bool,
    },
    /// Run the bridge (foreground).
    Run {
        /// Peer to connect to directly, `host` or `host:port` (repeatable). Use when mDNS
        /// discovery doesn't work on your network.
        #[arg(long = "peer", value_name = "HOST[:PORT]")]
        peers: Vec<String>,
        /// TCP port to listen on (default: from config, else 47321).
        #[arg(long)]
        port: Option<u16>,
        /// Don't advertise or browse with mDNS; only use --peer.
        #[arg(long)]
        no_discovery: bool,
        /// macOS: keep clips received from other machines on this Mac, instead of also
        /// offering them to your iPhone via Universal Clipboard.
        #[arg(long)]
        local_only: bool,
        /// Largest clip to send or accept, in MiB.
        #[arg(long, default_value_t = 32)]
        max_mb: usize,
    },
    /// Print this group's pairing code.
    Code,
    /// Check configuration and clipboard access.
    Doctor,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,mdns_sd=warn".into()),
        )
        .with_target(false)
        .init();
    let cli = Cli::parse();
    let path = match cli.config {
        Some(p) => p,
        None => config::default_path()?,
    };
    match cli.cmd {
        Cmd::Init { name, force } => {
            let cfg = Config::new(&Secret::generate(), name);
            cfg.save(&path, force)?;
            println!("Created {}\n", path.display());
            println!("On each other machine, run:\n\n    clipbridge join {}\n", cfg.code);
            println!("Then start the bridge everywhere with `clipbridge run`.");
            println!("Keep the code private: anyone with it can read your clipboard.");
        }
        Cmd::Join { code, name, force } => {
            let cfg = Config::new(&Secret::from_code(&code)?, name);
            cfg.save(&path, force)?;
            println!("Joined. Saved {}\nStart the bridge with `clipbridge run`.", path.display());
        }
        Cmd::Code => println!("{}", load(&path)?.code),
        Cmd::Doctor => doctor(&path),
        Cmd::Run {
            peers,
            port,
            no_discovery,
            local_only,
            max_mb,
        } => {
            let node_cfg = node_config(&path, peers, port, !no_discovery, max_mb)?;
            tokio::runtime::Runtime::new()?.block_on(run(node_cfg, WriteOptions { local_only }))?;
        }
    }
    Ok(())
}

fn load(path: &Path) -> Result<Config> {
    if !path.exists() {
        bail!(
            "no config at {} — run `clipbridge init` on one machine and `clipbridge join <code>` on the others",
            path.display()
        );
    }
    Config::load(path)
}

/// The group can come from `CLIPBRIDGE_CODE` (handy for containers) or the config file.
fn node_config(
    path: &Path,
    peers: Vec<String>,
    port: Option<u16>,
    discovery: bool,
    max_mb: usize,
) -> Result<NodeConfig> {
    let (secret, device_id, name, cfg_port) = match std::env::var("CLIPBRIDGE_CODE") {
        Ok(code) => (
            Secret::from_code(&code).context("CLIPBRIDGE_CODE")?,
            config::new_device_id(),
            config::default_name(),
            DEFAULT_PORT,
        ),
        Err(_) => {
            let cfg = load(path)?;
            (cfg.secret()?, cfg.device_id()?, cfg.name, cfg.port)
        }
    };
    Ok(NodeConfig {
        secret,
        device_id,
        name,
        listen: SocketAddr::from(([0, 0, 0, 0], port.unwrap_or(cfg_port))),
        static_peers: peers,
        discovery,
        max_clip: max_mb * 1024 * 1024,
        poll_interval: Duration::from_millis(250),
    })
}

async fn run(cfg: NodeConfig, opts: WriteOptions) -> Result<()> {
    let name = cfg.name.clone();
    let discovery = cfg.discovery;
    let node = Node::start(cfg, Box::new(move || clipboard::platform_backend(opts))).await?;
    info!(
        "clipbridge {} running as \"{name}\" on {} (clipboard: {}, discovery: {})",
        env!("CARGO_PKG_VERSION"),
        node.local_addr,
        node.backend,
        if discovery { "mDNS" } else { "off" },
    );
    #[cfg(target_os = "macos")]
    info!(
        "iPhone/iPad copies arrive here via Universal Clipboard and are forwarded{}",
        if opts.local_only { "" } else { "; clips from other machines are offered back to them" }
    );
    tokio::signal::ctrl_c().await?;
    info!("shutting down");
    drop(node);
    Ok(())
}

fn doctor(path: &Path) {
    println!("clipbridge {} on {}", env!("CARGO_PKG_VERSION"), std::env::consts::OS);
    match load(path) {
        Ok(cfg) => println!(
            "config: {} (name \"{}\", port {})",
            path.display(),
            cfg.name,
            cfg.port
        ),
        Err(e) => println!("config: {e:#}"),
    }
    match clipboard::platform_backend(WriteOptions::default()) {
        Ok(mut b) => {
            println!("clipboard: {} — opened OK", b.name());
            for line in b.diagnostics().into_iter().chain(b.startup_warnings()) {
                println!("  {line}");
            }
        }
        Err(e) => println!("clipboard: FAILED to open: {e:#}"),
    }
    #[cfg(target_os = "macos")]
    mac_handoff_status();
}

#[cfg(target_os = "macos")]
fn mac_handoff_status() {
    let read = |key: &str| {
        std::process::Command::new("defaults")
            .args(["-currentHost", "read", "com.apple.coreservices.useractivityd", key])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let show = |v: &Option<String>| match v.as_deref() {
        Some("1") => "on",
        Some("0") => "OFF",
        _ => "not set (system default: on)",
    };
    let adv = read("ActivityAdvertisingAllowed");
    let recv = read("ActivityReceivingAllowed");
    println!("Handoff (Universal Clipboard): advertising {}, receiving {}", show(&adv), show(&recv));
    if adv.as_deref() == Some("0") || recv.as_deref() == Some("0") {
        println!(
            "  → turn on System Settings → General → AirDrop & Handoff → \"Allow Handoff between \
             this Mac and your iCloud devices\" (and Handoff on the iPhone), or iPhone copies won't reach this Mac"
        );
    }
}
