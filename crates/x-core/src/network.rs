//! Unified network model: interfaces, addresses, routes, DNS and counters.

use serde::{Deserialize, Serialize};
use std::net::IpAddr;

/// Operational state of a network interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterfaceState {
    /// Up and carrying traffic.
    Up,
    /// Down.
    Down,
    /// Present but not connected (no carrier).
    Dormant,
    /// State could not be determined.
    Unknown,
}

/// A network interface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InterfaceInfo {
    /// Platform interface name (`en0`, `eth0`, `Ethernet`).
    pub name: String,
    /// Human friendly description when available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// MAC / hardware address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mac_address: Option<String>,
    /// Operational state.
    pub state: InterfaceState,
    /// MTU.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mtu: Option<u32>,
    /// Link layer speed in bits per second, when the platform reports one.
    ///
    /// This is the negotiated link speed, not a bandwidth measurement:
    /// Wi-Fi and virtual adapters either omit it or report the medium.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link_speed_bps: Option<u64>,
    /// Traffic counters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub received_bytes: Option<u64>,
    /// Traffic counters.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transmitted_bytes: Option<u64>,
}

/// An IP address bound to an interface.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AddressInfo {
    /// Interface the address belongs to.
    pub interface: String,
    /// The address itself.
    pub address: IpAddr,
    /// Network prefix length.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix_len: Option<u8>,
    /// Whether the address came from DHCP.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dhcp: Option<bool>,
}

/// A routing table entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteInfo {
    /// Destination network, `None` for the default route.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub destination: Option<String>,
    /// Next hop, `None` for on-link routes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gateway: Option<IpAddr>,
    /// Outgoing interface.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interface: Option<String>,
    /// Route metric / priority.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metric: Option<u64>,
}

/// A configured DNS resolver.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DnsServer {
    /// Resolver address.
    pub address: IpAddr,
    /// Interface the resolver was learned on, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interface: Option<String>,
}

/// The platform resolver configuration, grouped per nameserver.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DnsConfig {
    /// Resolvers in priority order.
    pub servers: Vec<DnsServer>,
    /// Per-resolver search domains.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub search_domains: Vec<String>,
}

impl DnsConfig {
    /// Drop repeated resolvers and search domains, keeping the original order.
    ///
    /// Platforms report the same resolver once per scope or per interface
    /// (`scutil --dns` does this routinely), and a user reading a list of
    /// resolvers should not have to notice that.
    pub fn deduped(mut self) -> Self {
        let mut seen = std::collections::HashSet::new();
        self.servers.retain(|server| seen.insert(server.address));
        let mut seen_domains = std::collections::HashSet::new();
        self.search_domains
            .retain(|domain| seen_domains.insert(domain.clone()));
        self
    }
}

/// Network capability, normalized across platforms.
pub trait NetworkManager: Send + Sync {
    /// List interfaces.
    fn interfaces(&self) -> crate::error::Result<Vec<InterfaceInfo>>;

    /// List IP addresses with their interface.
    fn addresses(&self) -> crate::error::Result<Vec<AddressInfo>>;

    /// List routes.
    fn routes(&self) -> crate::error::Result<Vec<RouteInfo>>;

    /// Platform resolver configuration.
    fn dns(&self) -> crate::error::Result<DnsConfig>;

    /// All sockets, including ones without an owning process (ICMP, raw).
    fn raw_connections(&self) -> crate::error::Result<Vec<crate::port::PortInfo>> {
        Ok(Vec::new())
    }

    /// Forward resolution through the platform resolver.
    ///
    /// Adapters that have no native resolver call report `unsupported`, so a
    /// frontend can distinguish "no records" from "cannot ask".
    fn resolve(&self, host: &str) -> crate::error::Result<Vec<IpAddr>> {
        let _ = host;
        Err(crate::Error::unsupported(
            "this platform adapter cannot resolve names",
        ))
    }

    /// Reverse (address to name) resolution.
    fn reverse_dns(&self, address: IpAddr) -> crate::error::Result<String> {
        let _ = address;
        Err(crate::Error::unsupported(
            "this platform adapter cannot reverse-resolve addresses",
        ))
    }

    /// Ask the platform resolver to drop its cached records.
    fn flush_dns_cache(&self) -> crate::error::Result<()> {
        Err(crate::Error::unsupported(
            "this platform adapter cannot flush the DNS cache",
        ))
    }

    /// Send ICMP echo requests and summarize the replies.
    fn ping(&self, request: &PingRequest) -> crate::error::Result<PingSummary> {
        let _ = request;
        Err(crate::Error::unsupported(
            "this platform adapter cannot send ICMP echoes",
        ))
    }

    /// Trace the path to `address`, one TTL at a time, at most `max_hops` far.
    fn trace(
        &self,
        address: IpAddr,
        max_hops: u32,
        timeout_ms: u32,
    ) -> crate::error::Result<Vec<TraceHop>> {
        let _ = (address, max_hops, timeout_ms);
        Err(crate::Error::unsupported(
            "this platform adapter cannot trace routes",
        ))
    }
}

/// One `x net ping` invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PingRequest {
    /// Resolved destination.
    pub address: IpAddr,
    /// Number of echo requests to send.
    pub count: u32,
    /// Per-request timeout in milliseconds.
    pub timeout_ms: u32,
}

/// One echo reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PingReply {
    /// Sequence number of the request this answers.
    pub sequence: u32,
    /// The address that answered, which may differ from the destination when
    /// a load balancer or anycast endpoint replies.
    pub address: IpAddr,
    /// Round trip time in milliseconds.
    pub rtt_ms: f64,
    /// Hop limit of the reply, when the platform reports it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<u8>,
}

/// The result of a whole ping run, in the order the replies arrived.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PingSummary {
    /// Destination of the run.
    pub address: IpAddr,
    /// Requests that were sent.
    pub transmitted: u32,
    /// Replies, sorted by sequence.
    pub replies: Vec<PingReply>,
}

impl PingSummary {
    /// Requests that got no reply.
    pub fn lost(&self) -> u32 {
        self.transmitted.saturating_sub(self.replies.len() as u32)
    }

    /// Fastest reply, if any.
    pub fn min_ms(&self) -> Option<f64> {
        self.rtts().into_iter().reduce(f64::min)
    }

    /// Slowest reply, if any.
    pub fn max_ms(&self) -> Option<f64> {
        self.rtts().into_iter().reduce(f64::max)
    }

    /// Mean round trip time, if any.
    pub fn avg_ms(&self) -> Option<f64> {
        let rtts = self.rtts();
        if rtts.is_empty() {
            return None;
        }
        Some(rtts.iter().sum::<f64>() / rtts.len() as f64)
    }

    fn rtts(&self) -> Vec<f64> {
        self.replies.iter().map(|reply| reply.rtt_ms).collect()
    }
}

/// One hop of a traceroute run: `address` is `None` when the hop timed out.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceHop {
    /// One-based hop position in the path.
    pub index: u32,
    /// Router that answered, when any did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<IpAddr>,
    /// Round trip time of the first reply, when there was one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rtt_ms: Option<f64>,
    /// `true` when this hop is the destination itself.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reached: bool,
}

impl TraceHop {
    /// `true` when nothing answered at this position.
    pub fn is_timeout(&self) -> bool {
        self.address.is_none()
    }
}

/// Format a link speed for display, rounding to the conventional names.
///
/// Link speeds are negotiated at powers-of-ten boundaries (`100 Mbps`,
/// `1 Gbps`), so a value within 2 % of such a boundary is printed as the
/// boundary: a NIC reporting `999_920_000` bit/s is a gigabit link.
pub fn format_link_speed(bits_per_second: u64) -> String {
    if bits_per_second == 0 {
        return "0 bps".to_string();
    }
    const UNITS: [(&str, u64); 4] = [
        ("Gbps", 1_000_000_000),
        ("Mbps", 1_000_000),
        ("Kbps", 1_000),
        ("bps", 1),
    ];
    for (unit, scale) in UNITS {
        let value = bits_per_second as f64 / scale as f64;
        // A gigabit link measured as 999.92 Mbps is still a gigabit link:
        // accept the unit slightly below 1 so the snap below can lift it.
        if value < 0.98 {
            continue;
        }
        let snapped = nearest_round(value);
        let display = if (snapped - value).abs() / value < 0.02 {
            snapped
        } else {
            value
        };
        return if display == display.trunc() {
            format!("{} {unit}", display as u64)
        } else {
            format!("{display:.1} {unit}")
        };
    }
    unreachable!("the bps unit has scale 1")
}

/// The nearest `n * 10^k` with a single leading digit, e.g. 1.2 -> 1,
/// 0.98 -> 1, 2.5 -> 2. Used only for snapping to conventional link names.
fn nearest_round(value: f64) -> f64 {
    let magnitude = 10f64.powf(value.log10().floor());
    (value / magnitude).round() * magnitude
}

/// Default route detection shared by adapters.
pub fn is_default_route(route: &RouteInfo) -> bool {
    match route.destination.as_deref() {
        None => true,
        Some("0.0.0.0/0") | Some("::/0") => true,
        Some(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_speeds_are_snapped_to_conventional_names() {
        assert_eq!(format_link_speed(1_000_000_000), "1 Gbps");
        // A gigabit NIC reports a hair under the round number.
        assert_eq!(format_link_speed(999_920_000), "1 Gbps");
        assert_eq!(format_link_speed(100_000_000), "100 Mbps");
        assert_eq!(format_link_speed(1_000_000_000_000), "1000 Gbps");
        // Values away from a boundary are shown as they are.
        assert_eq!(format_link_speed(1_250_000_000), "1.2 Gbps");
        assert_eq!(format_link_speed(0), "0 bps");
    }

    #[test]
    fn ping_summary_statistics() {
        let reply = |sequence: u32, rtt_ms: f64| PingReply {
            sequence,
            address: "127.0.0.1".parse().unwrap(),
            rtt_ms,
            ttl: Some(64),
        };
        let summary = PingSummary {
            address: "127.0.0.1".parse().unwrap(),
            transmitted: 3,
            replies: vec![reply(1, 12.5), reply(2, 7.5)],
        };
        assert_eq!(summary.lost(), 1);
        assert_eq!(summary.min_ms(), Some(7.5));
        assert_eq!(summary.max_ms(), Some(12.5));
        assert_eq!(summary.avg_ms(), Some(10.0));

        let silent = PingSummary {
            address: "127.0.0.1".parse().unwrap(),
            transmitted: 1,
            replies: Vec::new(),
        };
        assert_eq!(silent.lost(), 1);
        assert_eq!(silent.avg_ms(), None);
    }

    #[test]
    fn default_route_detection() {
        assert!(is_default_route(&RouteInfo {
            destination: None,
            gateway: Some("10.0.0.1".parse().unwrap()),
            interface: Some("eth0".into()),
            metric: Some(100),
        }));
        assert!(!is_default_route(&RouteInfo {
            destination: Some("192.168.0.0/24".into()),
            gateway: None,
            interface: Some("eth0".into()),
            metric: None,
        }));
    }
}
