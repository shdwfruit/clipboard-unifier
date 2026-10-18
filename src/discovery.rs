//! LAN discovery via mDNS / DNS-SD (`_clipbridge._tcp.local.`).

use crate::proto::DeviceId;
use anyhow::Result;
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::collections::HashMap;
use std::net::SocketAddr;
use tokio::sync::mpsc;

pub const SERVICE: &str = "_clipbridge._tcp.local.";

pub enum Found {
    Peer {
        fullname: String,
        id: DeviceId,
        addrs: Vec<SocketAddr>,
    },
    Gone {
        fullname: String,
    },
}

/// Keeps the mDNS daemon alive; dropping it stops advertising and browsing.
pub struct Discovery {
    daemon: ServiceDaemon,
}

impl Drop for Discovery {
    fn drop(&mut self) {
        let _ = self.daemon.shutdown();
    }
}

/// Advertise this node and report same-group peers.
pub fn start(
    self_id: DeviceId,
    name: &str,
    port: u16,
    group: String,
) -> Result<(Discovery, mpsc::UnboundedReceiver<Found>)> {
    let daemon = ServiceDaemon::new()?;
    let id_hex = hex::encode(self_id);
    let instance: String = format!("{} ({})", name, &id_hex[..8])
        .chars()
        .filter(|c| *c != '.')
        .take(60)
        .collect();
    let host = format!("clipbridge-{}.local.", &id_hex[..12]);
    let props = HashMap::from([
        ("g".to_string(), group.clone()),
        ("id".to_string(), id_hex),
        ("v".to_string(), "1".to_string()),
    ]);
    let info = ServiceInfo::new(SERVICE, &instance, &host, "", port, props)?.enable_addr_auto();
    daemon.register(info)?;

    let events = daemon.browse(SERVICE)?;
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::Builder::new()
        .name("mdns-browse".into())
        .spawn(move || {
            while let Ok(event) = events.recv() {
                let found = match event {
                    ServiceEvent::ServiceResolved(svc) => {
                        let props = &svc.txt_properties;
                        if props.get_property_val_str("g") != Some(group.as_str()) {
                            continue;
                        }
                        let Some(id) = props
                            .get_property_val_str("id")
                            .and_then(|h| hex::decode(h).ok())
                            .and_then(|b| DeviceId::try_from(b).ok())
                        else {
                            continue;
                        };
                        if id == self_id {
                            continue;
                        }
                        let mut addrs: Vec<SocketAddr> = svc
                            .addresses
                            .iter()
                            .map(|a| a.to_ip_addr())
                            // Loopback would point back at ourselves (everyone uses the same port).
                            .filter(|ip| ip.is_ipv4() && !ip.is_loopback())
                            .map(|ip| SocketAddr::new(ip, svc.port))
                            .collect();
                        addrs.sort();
                        Found::Peer {
                            fullname: svc.fullname.clone(),
                            id,
                            addrs,
                        }
                    }
                    ServiceEvent::ServiceRemoved(_, fullname) => Found::Gone { fullname },
                    _ => continue,
                };
                if tx.send(found).is_err() {
                    break;
                }
            }
        })?;
    Ok((Discovery { daemon }, rx))
}
