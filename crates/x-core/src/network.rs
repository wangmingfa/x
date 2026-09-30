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
