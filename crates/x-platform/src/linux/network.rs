//! Linux network adapter: shared `getifaddrs` for interfaces and addresses,
//! `/proc/net/route` and `/proc/net/ipv6_route` for the routing table,
//! `/etc/resolv.conf` for the resolver configuration, and the shared POSIX
//! probes for resolution, ping and traceroute.

use crate::common::{ifaddrs, netdiag, netprobe};
use std::net::IpAddr;
use x_core::error::{Error, Result};
use x_core::network::{
    AddressInfo, DnsConfig, DnsServer, InterfaceInfo, NetworkManager, PingRequest, PingSummary,
    RouteInfo, TraceHop,
};

/// Reads Linux network state.
#[derive(Debug, Default)]
pub struct LinuxNetwork;

impl LinuxNetwork {
    /// Create the adapter.
    pub fn new() -> Self {
        Self
    }
}

impl NetworkManager for LinuxNetwork {
    fn interfaces(&self) -> Result<Vec<InterfaceInfo>> {
        ifaddrs::interfaces()
    }

    fn addresses(&self) -> Result<Vec<AddressInfo>> {
        ifaddrs::addresses()
    }

    fn routes(&self) -> Result<Vec<RouteInfo>> {
        let mut routes = Vec::new();
        if let Some(raw) = read_optional("/proc/net/route") {
            routes.extend(parse_route(&raw));
        }
        if let Some(raw) = read_optional("/proc/net/ipv6_route") {
            routes.extend(parse_ipv6_route(&raw));
        }
        if routes.is_empty() {
            return Err(Error::system(
                "neither /proc/net/route nor /proc/net/ipv6_route could be read",
            ));
        }
        Ok(routes)
    }

    fn dns(&self) -> Result<DnsConfig> {
        let config = match read_optional("/etc/resolv.conf") {
            Some(raw) => parse_resolv_conf(&raw),
            None => DnsConfig::default(),
        };
        if config.servers.is_empty() {
            return Err(Error::system(
                "no nameserver found in /etc/resolv.conf".to_string(),
            ));
        }
        Ok(config.deduped())
    }

    fn resolve(&self, host: &str) -> Result<Vec<IpAddr>> {
        netprobe::resolve(host)
    }

    fn reverse_dns(&self, address: IpAddr) -> Result<String> {
        netprobe::reverse_dns(address)
    }

    /// Flush through whichever resolver cache the distribution runs:
    /// `systemd-resolved` answers to `resolvectl`, its predecessor to
    /// `systemd-resolve`, and glibc-only hosts cache in `nscd`.
    fn flush_dns_cache(&self) -> Result<()> {
        use crate::sys;
        let mut last_failure = None;
        for (program, args) in [
            ("resolvectl", &["flush-caches"][..]),
            ("systemd-resolve", &["--flush-caches"][..]),
            ("nscd", &["-i", "hosts"][..]),
        ] {
            match sys::run_command(program, args) {
                Ok(_) => return Ok(()),
                // Not installed: try the next client. Present but failing
                // (resolved not running): remember why, then try the next.
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(err) => last_failure = Some(format!("{program} failed: {err}")),
            }
        }
        match last_failure {
            Some(detail) => Err(Error::system(detail)),
            None => Err(Error::unsupported(
                "no DNS cache client found (resolvectl, systemd-resolve or nscd)",
            )),
        }
    }

    fn ping(&self, request: &PingRequest) -> Result<PingSummary> {
        netprobe::ping(request)
    }

    fn trace(&self, address: IpAddr, max_hops: u32, timeout_ms: u32) -> Result<Vec<TraceHop>> {
        netprobe::trace(address, max_hops, timeout_ms)
    }

    fn tls_info(&self, host: &str, port: u16, timeout_ms: u64) -> Result<x_core::netdiag::TlsInfo> {
        netdiag::tls_info(host, port, timeout_ms)
    }

    fn http_probe(
        &self,
        url: &str,
        method: &str,
        timeout_ms: u64,
    ) -> Result<x_core::netdiag::HttpResponse> {
        netdiag::http_probe(url, method, timeout_ms)
    }
}

fn read_optional(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// Parse the IPv4 routing table.
///
/// ```text
/// Iface  Destination  Gateway   Flags RefCnt Use Metric Mask      MTU Window IRTT
/// en0    00000000     0102A8C0 0003  0      0  100   00000000  0   0      0
/// ```
/// Every address is a host order u32 printed as hex, including the mask, which
/// is why the destination is re-rendered with a prefix length.
pub fn parse_route(raw: &str) -> Vec<RouteInfo> {
    raw.lines()
        .skip(1)
        .filter_map(|line| {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 8 {
                return None;
            }
            let interface = cols[0].to_string();
            // `/proc/net/route` prints each u32 as 8 hex chars in MEMORY order
            // (little-endian), so `0102A8C0` is 192.168.2.1.
            let destination = parse_hex_le(cols[1])?;
            let gateway = parse_hex_le(cols[2])?;
            let flags = u32::from_str_radix(cols[3], 16).ok()?;
            let metric = cols.get(6).and_then(|m| m.parse::<u64>().ok());
            let mask = parse_hex_le(cols[7])?;

            // RTF_REJECT and RTF_BLACKHOLE routes are unreachable on purpose.
            if flags & (0x0200 | 0x0040) != 0 {
                return None;
            }
            let prefix = prefix_length(mask);
            let address = std::net::IpAddr::V4(std::net::Ipv4Addr::from(destination));
            Some(RouteInfo {
                destination: format_destination(address, prefix),
                gateway: (flags & RTF_GATEWAY != 0 || gateway != 0)
                    .then(|| std::net::Ipv4Addr::from(gateway).into()),
                interface: Some(interface),
                metric,
            })
        })
        .collect()
}

/// Parse the IPv6 routing table.
///
/// ```text
/// dest_prefix                        plen src_prefix  src_plen next_hop  metric refcnt flags device
/// 00000000000000000000000000000000   80   0000...0000 80   0000...0000 01000000 00000003 00000001 0000003F en0
/// ```
/// Each address is four host order u32 words printed as hex.
pub fn parse_ipv6_route(raw: &str) -> Vec<RouteInfo> {
    raw.lines()
        .filter_map(|line| {
            // dest_prefix plen src_prefix src_plen next_hop metric refcnt flags device
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 9 {
                return None;
            }
            let prefix = cols[1].parse::<u8>().ok()?;
            let next_hop = parse_ipv6_hex(cols[4])?;
            let metric = cols.get(5).and_then(|m| m.parse::<u64>().ok());
            let flags = u32::from_str_radix(cols[7], 16).unwrap_or(0);
            if flags & 0x0200 != 0 {
                return None;
            }
            let interface = cols[8].to_string();
            let destination = std::net::Ipv6Addr::from(parse_ipv6_hex(cols[0])?);
            Some(RouteInfo {
                destination: format_destination(destination.into(), prefix),
                gateway: (next_hop != [0u8; 16]).then(|| next_hop.into()),
                interface: Some(interface),
                metric,
            })
        })
        .collect()
}

const RTF_GATEWAY: u32 = 0x2;

/// A little-endian hex u32 as printed by `/proc/net/*`.
fn parse_hex_le(raw: &str) -> Option<u32> {
    if raw.len() != 8 {
        return None;
    }
    let mut bytes = [0u8; 4];
    for (index, pair) in raw.as_bytes().chunks(2).enumerate() {
        bytes[index] = u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok()?;
    }
    Some(u32::from_le_bytes(bytes))
}

/// A 32 character IPv6 address made of four printed hex words.
///
/// The kernel prints each 4-byte word as the numeric value loaded in host
/// (little-endian) byte order, so `fe80::1` appears as `000080fe...01000000`.
fn parse_ipv6_hex(raw: &str) -> Option<[u8; 16]> {
    if raw.len() != 32 {
        return None;
    }
    let mut bytes = [0u8; 16];
    for (index, word) in raw.as_bytes().chunks(8).enumerate() {
        let word = u32::from_str_radix(std::str::from_utf8(word).ok()?, 16).ok()?;
        bytes[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    Some(bytes)
}

fn prefix_length(mask: u32) -> u8 {
    mask.count_ones() as u8
}

fn format_destination(address: std::net::IpAddr, prefix: u8) -> Option<String> {
    // An unspecified address is the catch-all route; showing `0.0.0.0/0` or
    // `::/80` as a destination adds nothing.
    match address {
        std::net::IpAddr::V4(ip) if ip.is_unspecified() => None,
        std::net::IpAddr::V6(ip) if ip.is_unspecified() => None,
        _ => Some(format!("{address}/{prefix}")),
    }
}

/// Parse `/etc/resolv.conf` into the unified resolver model.
pub fn parse_resolv_conf(raw: &str) -> DnsConfig {
    let mut config = DnsConfig::default();
    let mut search: Vec<String> = Vec::new();

    for line in raw.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let mut words = line.split_whitespace();
        match words.next() {
            Some("nameserver") => {
                if let Some(address) = words.next().and_then(|a| a.parse().ok()) {
                    config.servers.push(DnsServer {
                        address,
                        interface: None,
                    });
                }
            }
            Some("search") => search.extend(words.map(str::to_string)),
            Some("domain") => {
                if let Some(domain) = words.next() {
                    search.push(domain.to_string());
                }
            }
            _ => {}
        }
    }

    config.servers.dedup_by_key(|server| server.address);
    config.search_domains = search;
    config
}

/// Trait object helper.
pub fn manager() -> std::sync::Arc<dyn NetworkManager> {
    std::sync::Arc::new(LinuxNetwork::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    const ROUTE: &str =
        "Iface\tDestination\tGateway\t\tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\n\
        en0\t00000000\t0102A8C0\t0003\t0\t0\t100\t00000000\t0\t0\t0\n\
        en0\t0002A8C0\t00000000\t0001\t0\t0\t0\t00FFFFFF\t0\t0\t0\n\
        lo0\t011000A8\t00000000\t0001\t0\t0\t0\tFFFFFFFF\t0\t0\t0\n";

    const ROUTE6: &str = "dest_prefix                        plen src_prefix                         src_plen next_hop                           metric refcnt      flags   device\n\
        00000000000000000000000000000000   80   00000000000000000000000000000000   80   000080fe000000000000000001000000  00000003 00000001 000004001 en0\n\
        00000000000000000000000001000000   128  00000000000000000000000000000001  128  000080fe000000000000000001000000  00000000 00000002 0000000C lo0\n";

    #[test]
    fn ipv4_routes_carry_prefix_and_gateway() {
        let routes = parse_route(ROUTE);
        assert_eq!(routes.len(), 3);

        let default = &routes[0];
        assert!(default.destination.is_none(), "{default:?}");
        assert_eq!(
            default.gateway,
            Some(IpAddr::V4(Ipv4Addr::new(192, 168, 2, 1)))
        );
        assert_eq!(default.interface.as_deref(), Some("en0"));
        assert_eq!(default.metric, Some(100));
        assert!(x_core::network::is_default_route(default));

        assert_eq!(routes[1].destination.as_deref(), Some("192.168.2.0/24"));
        assert_eq!(routes[1].gateway, None, "on-link route has no gateway");
        assert!(!x_core::network::is_default_route(&routes[1]));
    }

    #[test]
    fn ipv6_routes_are_decoded_word_by_word() {
        let routes = parse_ipv6_route(ROUTE6);
        assert_eq!(routes.len(), 2);
        assert!(routes[0].destination.is_none(), "{routes:?}");
        assert_eq!(
            routes[0].gateway,
            Some("fe80::1".parse::<IpAddr>().unwrap())
        );
        assert_eq!(routes[0].metric, Some(3));
        assert_eq!(routes[1].destination.as_deref(), Some("::1/128"));
        assert_eq!(
            routes[1].gateway,
            Some("fe80::1".parse::<IpAddr>().unwrap())
        );
    }

    #[test]
    fn loopback_addresses_round_trip() {
        // /proc/net/ipv6_route prints four host order words, which is exactly the
        // form this parser inverts.
        let address = Ipv6Addr::LOCALHOST;
        let text: String = address
            .octets()
            .chunks(4)
            .map(|word| u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
            .map(|word| format!("{word:08x}"))
            .collect();
        assert_eq!(parse_ipv6_hex(&text), Some(address.octets()));
    }

    #[test]
    fn resolv_conf_parsing_collects_servers_and_search() {
        let raw = "# comment\nnameserver 10.0.0.53\nnameserver 10.0.0.54 # inline\nsearch corp.example.com example.com\noptions ndots:2\ndomain fallback.example.com\n";
        let config = parse_resolv_conf(raw);
        assert_eq!(config.servers.len(), 2);
        assert_eq!(
            config.servers[0].address,
            "10.0.0.53".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            config.search_domains,
            vec!["corp.example.com", "example.com", "fallback.example.com"]
        );
    }

    #[test]
    fn prefix_length_of_a_mask() {
        assert_eq!(prefix_length(0xFFFF_FFFF), 32);
        assert_eq!(prefix_length(0x00FF_FFFF), 24);
        assert_eq!(prefix_length(0), 0);
    }
}
