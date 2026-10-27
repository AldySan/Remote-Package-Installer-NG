//! Bounded LAN discovery of RPI-compatible consoles.
//!
//! The original PS4 payload has **no beacon** (`docs/BASELINE.md` §4.5), so
//! discovery is a bounded TCP sweep of the RPI ports across each interface's
//! real subnet, followed by API verification of every hit. Nothing is reported
//! as a console unless the RPI probes answer (`docs/ARCHITECTURE.md` §4.6).

use std::{
    collections::HashSet,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

use futures_util::stream::{self, StreamExt};

use crate::{
    model::{ConsoleCandidate, DiscoverySource},
    rpi::RpiClient,
    settings::Settings,
};

/// Default RPI-family ports, probed in this order (pkg-sender's order —
/// 12800 first, 9090 as the GoldHEN-family fallback).
pub const PROBE_PORTS: [u16; 2] = [12800, 9090];

const CONCURRENCY: usize = 64;

fn to_u32(ip: &Ipv4Addr) -> u32 {
    u32::from_be_bytes(ip.octets())
}

fn from_u32(v: u32) -> Ipv4Addr {
    Ipv4Addr::from(v.to_be_bytes())
}

/// One IPv4 subnet derived from a real interface address + netmask
/// (never a /24 assumption).
#[derive(Debug, Clone)]
pub struct LanNetwork {
    pub interface: String,
    pub network: u32,
    pub broadcast: u32,
    pub prefix: u32,
}

impl LanNetwork {
    pub fn from_interface(name: &str, addr: &IpAddr, mask: &IpAddr) -> Option<Self> {
        let (addr, mask) = match (addr, mask) {
            (IpAddr::V4(a), IpAddr::V4(m)) => (a, m),
            _ => return None,
        };
        if addr.is_loopback() {
            return None;
        }
        let a = to_u32(addr);
        let m = to_u32(mask);
        Some(Self {
            interface: name.to_string(),
            network: a & m,
            broadcast: a | !m,
            prefix: m.count_ones(),
        })
    }

    /// Usable host addresses (skips network + broadcast; caps at `max`).
    pub fn hosts(&self, max: usize) -> Vec<IpAddr> {
        if self.prefix >= 31 {
            return vec![
                IpAddr::V4(from_u32(self.network)),
                IpAddr::V4(from_u32(self.broadcast)),
            ];
        }
        let mut out = Vec::new();
        let total = self
            .broadcast
            .saturating_sub(self.network)
            .saturating_sub(1);
        let n = total.min(max as u32);
        for i in 0..n {
            out.push(IpAddr::V4(from_u32(self.network + 1 + i)));
        }
        out
    }

    pub fn truncated(&self, max: usize) -> bool {
        if self.prefix >= 31 {
            return false;
        }
        ((self.broadcast as u64) - (self.network as u64) - 1) > max as u64
    }
}

/// Enumerate local IPv4 networks from real interfaces.
pub fn local_networks() -> Vec<LanNetwork> {
    let mut nets = Vec::new();
    if let Ok(ifaces) = if_addrs::get_if_addrs() {
        for iface in ifaces {
            let (ip, mask) = match iface.addr {
                if_addrs::IfAddr::V4(v4) => (IpAddr::V4(v4.ip), IpAddr::V4(v4.netmask)),
                if_addrs::IfAddr::V6(_) => continue,
            };
            if let Some(net) = LanNetwork::from_interface(&iface.name, &ip, &mask) {
                nets.push(net);
            }
        }
    }
    nets
}

/// Discover consoles on the LAN: remembered hosts first, then a bounded TCP
/// sweep + API verification. Never busy-polls; a single bounded pass.
pub async fn discover(settings: &Settings) -> Vec<ConsoleCandidate> {
    let mut candidates: Vec<ConsoleCandidate> = Vec::new();
    let mut seen_ips: HashSet<IpAddr> = HashSet::new();

    // 1. Remembered hosts (usually the answer).
    let remembered: Vec<SocketAddr> = settings.remembered.iter().map(|rc| rc.addr).collect();
    for addr in remembered {
        if seen_ips.insert(addr.ip())
            && let Some(c) = verify_addr(addr, DiscoverySource::Remembered).await
        {
            candidates.push(c);
        }
    }

    // 2. Bounded subnet sweep.
    if settings.discovery_enabled {
        let max_per_net = settings.discovery_max_hosts as usize;
        let mut targets: Vec<SocketAddr> = Vec::new();
        for net in local_networks() {
            for ip in net.hosts(max_per_net) {
                for port in PROBE_PORTS {
                    targets.push(SocketAddr::new(ip, port));
                }
            }
        }

        let timeout = settings.discovery_timeout;
        let hits: Vec<Option<SocketAddr>> =
            stream::iter(targets.into_iter().map(|addr| async move {
                match tokio::time::timeout(timeout, tokio::net::TcpStream::connect(addr)).await {
                    Ok(Ok(_)) => Some(addr),
                    _ => None,
                }
            }))
            .buffer_unordered(CONCURRENCY)
            .collect()
            .await;

        // Dedup by IP (first port hit wins), skipping already-verified ones.
        let mut ordered: Vec<SocketAddr> = Vec::new();
        let mut hit_ips: HashSet<IpAddr> = HashSet::new();
        for addr in hits.into_iter().flatten() {
            if seen_ips.contains(&addr.ip()) {
                continue;
            }
            if hit_ips.insert(addr.ip()) {
                ordered.push(addr);
            }
        }

        // 3. Verify each hit speaks the RPI dialect.
        for addr in ordered {
            if let Some(c) = verify_addr(addr, DiscoverySource::SubnetProbe).await {
                candidates.push(c);
            }
        }
    }

    candidates
}

/// Verify one address on the known ports; the first port that answers the
/// RPI probes wins.
pub async fn verify_addr(addr: SocketAddr, source: DiscoverySource) -> Option<ConsoleCandidate> {
    let ip = addr.ip();
    for port in PROBE_PORTS {
        let target = SocketAddr::new(ip, port);
        let client = RpiClient::new(target, Duration::from_millis(1500));
        if let Ok(probe) = client.probe().await {
            return Some(ConsoleCandidate {
                addr: target,
                kind: probe.kind,
                source,
            });
        }
    }
    None
}
