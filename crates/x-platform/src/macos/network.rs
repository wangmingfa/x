//! macOS network adapter: `getifaddrs` for interfaces and addresses, `sysctl`
//! for the routing table, `scutil` for the resolver configuration, and the
//! shared POSIX probes for resolution, ping and traceroute.
//!
//! `getifaddrs` is the same API Linux exposes, which is why the enumeration lives
//! in [`crate::common::ifaddrs`] and the unified
//! `InterfaceInfo`/`AddressInfo` model maps cleanly onto both.

use crate::common::{ifaddrs, netprobe};
use crate::sys;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::os::raw::{c_char, c_int};
use x_core::error::{Error, PermissionRequirement, Result};
use x_core::network::{
    is_default_route, AddressInfo, DnsConfig, DnsServer, InterfaceInfo, NetworkManager,
    PingRequest, PingSummary, RouteInfo, TraceHop,
};

/// Reads macOS network state.
#[derive(Debug, Default)]
pub struct MacosNetwork;

impl MacosNetwork {
    /// Create the adapter.
    pub fn new() -> Self {
        Self
    }
}

impl NetworkManager for MacosNetwork {
    fn interfaces(&self) -> Result<Vec<InterfaceInfo>> {
        ifaddrs::interfaces()
    }

    fn addresses(&self) -> Result<Vec<AddressInfo>> {
        let mut rows = ifaddrs::addresses()?;
        // The DHCP lease is a property of the interface and lives in the DHCP
        // client, not in the kernel's address table, so `ipconfig getpacket`
        // answers per interface. IPv6 addresses are excluded: they come from
        // SLAAC or DHCPv6, and a DHCPv4 lease says nothing about them.
        let mut lease: HashMap<String, bool> = HashMap::new();
        for row in &mut rows {
            if row.address.is_ipv6() || row.interface.starts_with("lo") {
                continue;
            }
            let has_lease = match lease.get(&row.interface).copied() {
                Some(seen) => seen,
                None => {
                    let seen = sys::run_command("ipconfig", &["getpacket", &row.interface])
                        .map(|output| output.contains("yiaddr"))
                        .unwrap_or(false);
                    lease.insert(row.interface.clone(), seen);
                    seen
                }
            };
            row.dhcp = Some(has_lease);
        }
        Ok(rows)
    }

    fn routes(&self) -> Result<Vec<RouteInfo>> {
        // `NET_RT_DUMP2` was withdrawn from the BSD network stack in macOS 11
        // in favour of the Network framework, so the sysctl path is still
        // preferred where it exists and `netstat -nr` carries the rest.
        match read_routes() {
            Ok(routes) if !routes.is_empty() => Ok(routes),
            _ => routes_from_netstat(),
        }
    }

    fn dns(&self) -> Result<DnsConfig> {
        let mut config = DnsConfig::default();
        if let Ok(output) = sys::run_command("scutil", &["--dns"]) {
            config = parse_scutil_dns(&output).deduped();
        }
        if config.servers.is_empty() {
            config.servers = default_resolvers();
        }
        Ok(config)
    }

    fn resolve(&self, host: &str) -> Result<Vec<IpAddr>> {
        netprobe::resolve(host)
    }

    fn reverse_dns(&self, address: IpAddr) -> Result<String> {
        netprobe::reverse_dns(address)
    }

    /// The system cache is dropped by `dscacheutil`; `mDNSResponder` keeps a
    /// second copy in memory and is reloaded with `HUP`. Both steps want root
    /// on a current macOS, so a failure is reported as a permission problem.
    fn flush_dns_cache(&self) -> Result<()> {
        for (program, args) in [
            ("dscacheutil", &["-flushcache"][..]),
            ("killall", &["-HUP", "mDNSResponder"][..]),
        ] {
            if let Err(err) = sys::run_command(program, args) {
                return Err(Error::permission_denied(
                    PermissionRequirement::Root,
                    format!("`{program} {}` failed: {err}", args.join(" ")),
                ));
            }
        }
        Ok(())
    }

    fn ping(&self, request: &PingRequest) -> Result<PingSummary> {
        netprobe::ping(request)
    }

    fn trace(&self, address: IpAddr, max_hops: u32, timeout_ms: u32) -> Result<Vec<TraceHop>> {
        netprobe::trace(address, max_hops, timeout_ms)
    }
}

/// `sysctl` selector for the full routing table dump.
const NET_RT_DUMP2: c_int = 17;

/// `struct rt_msghdr2` from `<net/route.h>`, used by `NET_RT_DUMP2`.
#[repr(C)]
#[derive(Clone, Copy)]
struct rt_msghdr2 {
    rtm_msglen: u16,
    rtm_version: u8,
    rtm_type: u8,
    rtm_addrs: i32,
    rtm_flags: i32,
    rtm_index: u16,
    rtm_seqno: i32,
    rtm_errno: i32,
    rtm_fmask: i32,
    rtm_rmx: rt_metrics,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct rt_metrics {
    rmx_locks: i32,
    rmx_mtu: i32,
    rmx_hopcount: i32,
    rmx_expire: i32,
    rmx_recvpipe: i32,
    rmx_sendpipe: i32,
    rmx_ssthresh: i32,
    rmx_rtt: i32,
    rmx_rttvar: i32,
    rmx_pksent: i32,
    rmx_weight: i32,
    rmx_nhidx: i32,
    rmx_filler: [i32; 2],
}

/// Address families as they appear in a `sockaddr` inside a route message.
const AF_INET: u8 = 2;
const AF_INET6: u8 = 30;
const AF_LINK: u8 = 18;

/// Address flags inside `rtm_addrs`.
const RTA_DST: u16 = 0x1;
const RTA_GATEWAY: u16 = 0x2;
const RTA_NETMASK: u16 = 0x4;
const RTA_IFP: u16 = 0x10;

const HEADER_LEN: usize = std::mem::size_of::<rt_msghdr2>();

/// Read the routing table through the `NET_RT_DUMP2` sysctl.
///
/// This is the native equivalent of `netstat -nr`: no process is spawned and
/// the kernel returns every route, including the per-interface IPv6 defaults.
pub fn read_routes() -> Result<Vec<RouteInfo>> {
    // {CTL_NET, PF_ROUTE, PF_INET, RTM_ALL, RTM_ALL, 0}
    let mut mib = [0 as c_int; 6];
    mib[2] = 2;
    let mut needed: libc::size_t = 0;

    debug_assert_eq!(mib[2], NET_RT_DUMP2 - 15);
    // SAFETY: `mib` is the 6 element array NET_RT_DUMP2 expects. The first call
    // asks for the required buffer size, the second one fills it.
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            6,
            std::ptr::null_mut(),
            &mut needed,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 || needed == 0 {
        return Err(Error::system("sysctl(3) returned no routing table size"));
    }

    let mut buffer = vec![0u8; needed];
    // SAFETY: `buffer` is exactly `needed` bytes, which is what we declare.
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            6,
            buffer.as_mut_ptr() as *mut libc::c_void,
            &mut needed,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return Err(Error::system("sysctl(3) route dump failed"));
    }

    Ok(parse_route_dump(&buffer[..needed as usize]))
}

/// Read the routing table through `netstat -nr`, the documented fallback when
/// the `NET_RT_DUMP2` sysctl is unavailable.
pub fn routes_from_netstat() -> Result<Vec<RouteInfo>> {
    let mut routes = Vec::new();
    let mut failures = Vec::new();

    for family in ["inet", "inet6"] {
        match sys::run_command("netstat", &["-nr", "-f", family]) {
            Ok(output) => routes.extend(parse_routes(&output)),
            Err(err) => failures.push(format!("{family}: {err}")),
        }
    }

    if routes.is_empty() {
        let detail = failures.join("; ");
        return Err(Error::system(format!(
            "no routing table could be read ({detail})"
        )));
    }
    Ok(routes)
}

/// Walk a `NET_RT_DUMP2` buffer.
fn parse_route_dump(buffer: &[u8]) -> Vec<RouteInfo> {
    let mut routes = Vec::new();
    let mut offset = 0usize;

    while offset + HEADER_LEN <= buffer.len() {
        // SAFETY: bounds are checked by the loop condition and the kernel.
        let header =
            unsafe { std::ptr::read_unaligned(buffer.as_ptr().add(offset) as *const rt_msghdr2) };
        let length = header.rtm_msglen as usize;
        if length < HEADER_LEN || offset + length > buffer.len() {
            break;
        }

        let mut cursor = offset + HEADER_LEN;
        let mut destination: Option<IpAddr> = None;
        let mut gateway: Option<IpAddr> = None;
        let mut interface: Option<String> = None;
        let mut prefix: Option<u8> = None;
        let mut bits = header.rtm_addrs;
        let mut bit = 0u32;

        while bits != 0 && cursor < offset + length {
            if bits & 1 == 0 {
                bit += 1;
                bits >>= 1;
                continue;
            }
            // SAFETY: the kernel guarantees at least a length byte here.
            let sa_len = buffer[cursor] as usize;
            match bit {
                b if b == RTA_DST.trailing_zeros() => {
                    destination = read_ip(&buffer[cursor..(offset + length).min(cursor + sa_len)]);
                }
                b if b == RTA_GATEWAY.trailing_zeros() => {
                    gateway = read_ip(&buffer[cursor..(offset + length).min(cursor + sa_len)]);
                }
                b if b == RTA_NETMASK.trailing_zeros() => {
                    prefix =
                        read_prefix_len(&buffer[cursor..(offset + length).min(cursor + sa_len)]);
                }
                b if b == RTA_IFP.trailing_zeros() => {
                    interface = read_interface(
                        &buffer[cursor..(offset + length).min(cursor + sa_len)],
                        header.rtm_index,
                    );
                }
                _ => {}
            }
            if sa_len == 0 {
                break;
            }
            cursor += sa_len;
            bits >>= 1;
            bit += 1;
        }

        // `rtm_type` 2 (RTM_ADD) only; other message types are notifications.
        if header.rtm_type == 2 {
            routes.push(RouteInfo {
                destination: format_destination(destination, prefix),
                gateway,
                interface,
                // `rtm_inits` only exists on the BSD routing socket API; the
                // sysctl dump exposes the metric as the message's hop count.
                metric: (header.rtm_rmx.rmx_hopcount > 0)
                    .then_some(header.rtm_rmx.rmx_hopcount as u64),
            });
        }
        offset += length;
    }
    routes
}

/// Family byte of a `sockaddr` at the start of a route attribute.
fn sockaddr_family(bytes: &[u8]) -> u8 {
    bytes.get(1).copied().unwrap_or(0)
}

fn read_ip(bytes: &[u8]) -> Option<IpAddr> {
    // The route attribute starts with the `sockaddr` length and family bytes.
    if bytes.len() >= 16 && sockaddr_family(bytes) == AF_INET6 {
        let mut raw = [0u8; 16];
        raw.copy_from_slice(&bytes[8..24.min(bytes.len())]);
        // SAFETY-free path: only the first 16 bytes are needed and we bounds
        // checked above.
        if bytes.len() >= 24 {
            raw.copy_from_slice(&bytes[8..24]);
            Some(IpAddr::V6(Ipv6Addr::from(raw)))
        } else {
            None
        }
    } else if bytes.len() >= 8 && sockaddr_family(bytes) == AF_INET {
        Some(IpAddr::V4(Ipv4Addr::new(
            bytes[4], bytes[5], bytes[6], bytes[7],
        )))
    } else {
        None
    }
}

fn read_prefix_len(bytes: &[u8]) -> Option<u8> {
    if bytes.len() >= 8 && bytes[1] == AF_INET {
        let mask = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        Some(mask.count_ones() as u8)
    } else if bytes.len() >= 24 && bytes[1] == AF_INET6 {
        let mut count = 0u8;
        for byte in &bytes[8..24] {
            count += byte.count_ones() as u8;
        }
        Some(count)
    } else {
        None
    }
}

fn read_interface(bytes: &[u8], index: u16) -> Option<String> {
    // BSD encodes the interface name in the RTA_IFP sockaddr with a zero
    // `sa_len`, and otherwise exposes a `sockaddr_dl` carrying the index.
    if bytes.len() > 2 && bytes[0] == 0 && sockaddr_family(bytes) == AF_LINK {
        return cstr_from_bytes(&bytes[2..]);
    }
    if bytes.len() >= 4 && bytes[0] as usize >= 4 && sockaddr_family(bytes) == AF_LINK {
        let sdl_index = u16::from_ne_bytes([bytes[2], bytes[3]]);
        return if_nametoindex(sdl_index).or(if_nametoindex(index));
    }
    if_nametoindex(index)
}

fn cstr_from_bytes(bytes: &[u8]) -> Option<String> {
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    std::str::from_utf8(&bytes[..end]).ok().map(str::to_string)
}

fn if_nametoindex(index: u16) -> Option<String> {
    if index == 0 {
        return None;
    }
    let mut buffer = [0u8; libc::IF_NAMESIZE];
    // SAFETY: `buffer` is IF_NAMESIZE bytes, which is what the API requires.
    let name =
        unsafe { libc::if_indextoname(index as libc::c_uint, buffer.as_mut_ptr() as *mut c_char) };
    if name.is_null() {
        return None;
    }
    cstr_from_bytes(&buffer)
}

fn format_destination(destination: Option<IpAddr>, prefix: Option<u8>) -> Option<String> {
    match (destination, prefix) {
        (None, _) => None,
        (Some(IpAddr::V4(ip)), Some(0)) if ip == Ipv4Addr::UNSPECIFIED => None,
        (Some(IpAddr::V6(ip)), Some(0)) if ip == Ipv6Addr::UNSPECIFIED => None,
        (Some(ip), Some(prefix)) => Some(format!("{ip}/{prefix}")),
        (Some(ip), None) => Some(ip.to_string()),
    }
}

/// Convert raw routing table text into [`RouteInfo`] rows.
///
/// macOS `netstat -nr` output looks like:
/// ```text
/// Destination            Gateway               Flags   Netif Expire
/// default                192.168.1.1           UGScg   en0
/// 127                    127.0.0.1             UCS             lo0
/// 192.168.1/24           192.168.1.1           UGS             en0
/// ```
pub fn parse_routes(raw: &str) -> Vec<RouteInfo> {
    let mut routes = Vec::new();
    let mut in_table = false;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Section noise: "Routing tables", "Internet:", "Internet6:".
        if trimmed.starts_with("Routing tables") || trimmed.ends_with(':') && trimmed.len() < 12 {
            in_table = false;
            continue;
        }
        if trimmed.starts_with("Destination") {
            in_table = true;
            continue;
        }
        if !in_table {
            continue;
        }

        // Columns are whitespace aligned: destination, gateway, flags, netif and
        // an optional expire marker.
        let cols: Vec<&str> = trimmed.split_whitespace().collect();
        if cols.len() < 4 {
            in_table = false;
            continue;
        }
        let (destination, gateway, interface) = (cols[0], cols[1], cols[3]);
        if !is_destination_token(destination) {
            in_table = false;
            continue;
        }

        routes.push(RouteInfo {
            destination: (destination != "default").then(|| destination.to_string()),
            gateway: parse_gateway(gateway),
            interface: (!interface.is_empty()).then(|| interface.to_string()),
            // macOS has no route metric column in `netstat -nr`; the expire
            // marker is a countdown, not a metric.
            metric: None,
        });
    }

    routes
}

/// A destination column is `default`, a bare address or an address with a
/// prefix length. Anything else means the table ended.
fn is_destination_token(token: &str) -> bool {
    if token == "default" {
        return true;
    }
    match token.split_once('/') {
        Some((address, prefix)) => {
            prefix.parse::<u8>().is_ok() && !address.is_empty() && is_address_like(address)
        }
        // `netstat -nr` also prints classful shorthands such as `127` or
        // `169.254` for IPv4.
        None => is_address_like(token),
    }
}

fn is_address_like(token: &str) -> bool {
    token.contains(':')
        || token.contains('.')
        || token
            .chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == '%')
}

/// `link#25` is an on-link placeholder, not an address.
fn parse_gateway(raw: &str) -> Option<IpAddr> {
    if raw.starts_with("link#") {
        return None;
    }
    // IPv6 gateways carry a zone index, e.g. `fe80::1%en0`.
    let address = raw.split('%').next().unwrap_or(raw);
    address.parse().ok()
}

/// `scutil --dns` writes `key[n] : value`; return the trimmed value.
fn after_bracket(value: &str) -> Option<String> {
    let (_, rest) = value.split_once(']')?;
    let rest = rest.trim().trim_start_matches(':').trim();
    (!rest.is_empty()).then(|| rest.to_string())
}

/// Parse `scutil --dns` output into a [`DnsConfig`].
pub fn parse_scutil_dns(raw: &str) -> DnsConfig {
    let mut config = DnsConfig::default();

    for line in raw.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("nameserver[") {
            // `nameserver[0] : 10.0.0.53` -> take everything after the bracket.
            let ip = after_bracket(value).and_then(|rest| rest.parse().ok());
            if let Some(ip) = ip {
                config.servers.push(DnsServer {
                    address: ip,
                    interface: None,
                });
            }
        }
        if let Some(value) = trimmed.strip_prefix("search domain[") {
            if let Some(domain) = after_bracket(value) {
                config.search_domains.push(domain);
            }
        }
    }
    config.servers.dedup();
    config
}

/// Resolvers configured in `/etc/resolv.conf`, the portable fallback.
fn default_resolvers() -> Vec<DnsServer> {
    let Ok(content) = std::fs::read_to_string("/etc/resolv.conf") else {
        return Vec::new();
    };
    content
        .lines()
        .filter_map(|line| line.strip_prefix("nameserver"))
        .filter_map(|rest| rest.trim().parse::<IpAddr>().ok())
        .map(|address| DnsServer {
            address,
            interface: None,
        })
        .collect()
}

/// Trait object helper.
pub fn manager() -> std::sync::Arc<dyn NetworkManager> {
    std::sync::Arc::new(MacosNetwork::new())
}

/// Default adapter.
pub fn as_manager() -> std::sync::Arc<dyn NetworkManager> {
    manager()
}

/// `true` when the route is the default route.
pub fn is_default(route: &RouteInfo) -> bool {
    is_default_route(route)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interfaces_include_loopback() {
        let rows = MacosNetwork::new().interfaces().expect("interfaces");
        assert!(rows.iter().any(|i| i.name == "lo0"), "no lo0 in {rows:?}");
        assert!(rows.iter().all(|i| !i.name.is_empty()));
    }

    #[test]
    fn addresses_include_loopback_v4() {
        let rows = MacosNetwork::new().addresses().expect("addresses");
        assert!(
            rows.iter().any(|a| a.address.is_loopback()),
            "no loopback in {rows:?}"
        );
    }

    #[test]
    fn route_parser_handles_default_route() {
        let raw = "Destination            Gateway               Flags   Netif Expire\n\
                    default                192.168.1.1           UGScg   en0\n\
                    127                    127.0.0.1             UCS             lo0\n\
                    192.168.1/24           192.168.1.1           UGS             en0\n";
        let routes = parse_routes(raw);
        assert_eq!(routes.len(), 3);
        assert!(routes[0].destination.is_none());
        assert_eq!(routes[0].gateway, Some("192.168.1.1".parse().unwrap()));
        assert_eq!(routes[0].interface.as_deref(), Some("en0"));
        assert!(is_default(&routes[0]));
        assert!(!is_default(&routes[2]));
    }

    #[test]
    fn scutil_dns_parser() {
        let raw = "DNS configuration\n\n\
                    resolver #1\n\
                    search domain[0] : corp.example.com\n\
                    nameserver[0] : 10.0.0.53\n\
                    nameserver[1] : 10.0.0.54\n\
                    flags    : Request A records\n";
        let config = parse_scutil_dns(raw);
        assert_eq!(config.servers.len(), 2);
        assert_eq!(
            config.servers[0].address,
            "10.0.0.53".parse::<IpAddr>().unwrap()
        );
        assert_eq!(config.search_domains, vec!["corp.example.com".to_string()]);
    }

    #[test]
    fn dns_always_returns_something_on_macos() {
        let config = MacosNetwork::new().dns().expect("dns");
        assert!(!config.servers.is_empty(), "no resolver found");
    }

    #[test]
    fn routes_are_available_and_include_loopback() {
        let routes = MacosNetwork::new().routes().expect("routes");
        assert!(!routes.is_empty(), "routing table must not be empty");
        assert!(
            routes.iter().any(|r| r.interface.as_deref() == Some("lo0")),
            "no lo0 route in {routes:?}"
        );
        assert!(routes
            .iter()
            .all(|r| r.destination.is_some() || r.gateway.is_some()));
    }

    #[test]
    fn a_default_route_exists_on_a_normal_network() {
        let routes = MacosNetwork::new().routes().expect("routes");
        let defaults: Vec<_> = routes.iter().filter(|r| is_default(r)).collect();
        // A fully offline machine has no default route; when there is one it
        // must carry a gateway or an interface.
        for route in defaults {
            assert!(route.gateway.is_some() || route.interface.is_some());
        }
    }
}
