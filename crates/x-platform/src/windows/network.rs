//! Windows network adapter: `GetAdaptersAddresses` for interfaces, addresses
//! and DNS servers, `route print` for the routing table.

use super::buffer::AlignedBuffer;
use crate::sys;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetAdaptersAddresses, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_MULTICAST, IP_ADAPTER_ADDRESSES_LH,
};
use windows_sys::Win32::Networking::WinSock::SOCKET_ADDRESS;
use x_core::error::{Error, Result};
use x_core::network::{
    AddressInfo, DnsConfig, DnsServer, InterfaceInfo, InterfaceState, NetworkManager, RouteInfo,
};

const AF_UNSPEC: u32 = 0;
const IF_OPER_STATUS_UP: i32 = 1;

/// Reads Windows network state.
#[derive(Debug, Default)]
pub struct WindowsNetwork;

impl WindowsNetwork {
    /// Create the adapter.
    pub fn new() -> Self {
        Self
    }
}

/// One adapter as `GetAdaptersAddresses` reports it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Adapter {
    /// Adapter name, the `\\Device\\NPF_{GUID}` style identifier.
    pub name: String,
    /// Human readable adapter name.
    pub friendly_name: String,
    /// Adapter description.
    pub description: String,
    /// Oper status.
    pub oper_status: i32,
    /// MTU.
    pub mtu: u32,
    /// Physical address.
    pub physical_address: Vec<u8>,
    /// Unicast addresses with their prefix lengths.
    pub addresses: Vec<(IpAddr, u8)>,
    /// DNS servers.
    pub dns_servers: Vec<IpAddr>,
    /// DNS suffix.
    pub dns_suffix: String,
}

impl Adapter {
    /// Translate into the unified interface model.
    pub fn interface(&self) -> InterfaceInfo {
        InterfaceInfo {
            name: if self.friendly_name.is_empty() {
                self.name.clone()
            } else {
                self.friendly_name.clone()
            },
            description: Some(self.description.clone()).filter(|d| !d.is_empty()),
            mac_address: format_mac(&self.physical_address),
            state: if self.oper_status == IF_OPER_STATUS_UP {
                InterfaceState::Up
            } else {
                InterfaceState::Down
            },
            mtu: (self.mtu > 0).then_some(self.mtu),
            received_bytes: None,
            transmitted_bytes: None,
        }
    }

    /// Translate the unicast addresses.
    pub fn address_rows(&self) -> Vec<AddressInfo> {
        let name = if self.friendly_name.is_empty() {
            self.name.clone()
        } else {
            self.friendly_name.clone()
        };
        self.addresses
            .iter()
            .map(|(address, prefix_len)| AddressInfo {
                interface: name.clone(),
                address: *address,
                prefix_len: Some(*prefix_len),
                dhcp: None,
            })
            .collect()
    }
}

impl NetworkManager for WindowsNetwork {
    fn interfaces(&self) -> Result<Vec<InterfaceInfo>> {
        Ok(adapters()?.iter().map(Adapter::interface).collect())
    }

    fn addresses(&self) -> Result<Vec<AddressInfo>> {
        Ok(adapters()?.iter().flat_map(Adapter::address_rows).collect())
    }

    fn routes(&self) -> Result<Vec<RouteInfo>> {
        // Windows has no route enumeration API for user mode, so `route print`
        // is the supported interface. It is the documented level 3 fallback.
        let output = sys::run_command("route", &["print", "-4"])
            .or_else(|_| sys::run_command("route", &["print"]))
            .map_err(|err| Error::system(format!("route print: {err}")))?;
        let routes = parse_route_print(&output);
        if routes.is_empty() {
            return Err(Error::system("route print produced no routes"));
        }
        Ok(routes)
    }

    fn dns(&self) -> Result<DnsConfig> {
        let adapters = adapters()?;
        let servers: Vec<DnsServer> = adapters
            .iter()
            .flat_map(|adapter| adapter.dns_servers.iter().copied())
            .map(|address| DnsServer {
                address,
                interface: None,
            })
            .collect();

        if servers.is_empty() {
            return Err(Error::system("no DNS server reported by any adapter"));
        }

        let search_domains: Vec<String> = adapters
            .iter()
            .filter(|adapter| !adapter.dns_suffix.is_empty())
            .map(|adapter| adapter.dns_suffix.clone())
            .collect();

        Ok(DnsConfig {
            servers,
            search_domains,
        }
        .deduped())
    }
}

/// Enumerate every adapter with its addresses and DNS servers.
pub fn adapters() -> Result<Vec<Adapter>> {
    // DNS servers are part of the answer, so `GAA_FLAG_SKIP_DNS_SERVER` must not be
    // set here; the flags only drop the address families we never render.
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST;
    let mut size: u32 = 0;
    // SAFETY: the first call sizes the buffer, the second one fills it.
    let rc = unsafe {
        GetAdaptersAddresses(
            AF_UNSPEC,
            flags,
            std::ptr::null(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    if size == 0 {
        return Err(Error::system(format!(
            "GetAdaptersAddresses reported no adapters (status {rc})"
        )));
    }

    let mut buffer = AlignedBuffer::zeroed(size as usize);
    // SAFETY: `buffer` is at least the size the API asked for, and is aligned for
    // the linked list it is asked to fill.
    let rc = unsafe {
        GetAdaptersAddresses(
            AF_UNSPEC,
            flags,
            std::ptr::null(),
            buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>(),
            &mut size,
        )
    };
    if rc != 0 {
        return Err(Error::system(format!(
            "GetAdaptersAddresses failed with status {rc}"
        )));
    }

    let mut adapters = Vec::new();
    let mut node = buffer.as_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
    while !node.is_null() {
        // SAFETY: the list is NUL terminated by the API and lives in `buffer`.
        let entry = unsafe { &*node };
        adapters.push(Adapter {
            name: cstr_to_string(entry.AdapterName),
            friendly_name: wide_to_string(entry.FriendlyName),
            description: wide_to_string(entry.Description),
            oper_status: entry.OperStatus,
            mtu: entry.Mtu,
            physical_address: entry.PhysicalAddress[..entry.PhysicalAddressLength.min(8) as usize]
                .to_vec(),
            addresses: unicast_addresses(entry),
            dns_servers: dns_servers(entry),
            dns_suffix: wide_to_string(entry.DnsSuffix),
        });
        node = entry.Next;
    }
    Ok(adapters)
}

fn unicast_addresses(entry: &IP_ADAPTER_ADDRESSES_LH) -> Vec<(IpAddr, u8)> {
    let mut addresses = Vec::new();
    let mut node = entry.FirstUnicastAddress;
    while !node.is_null() {
        // SAFETY: the chain is owned by the adapter buffer.
        let unicast = unsafe { &*node };
        if let Some(address) = socket_address(&unicast.Address) {
            addresses.push((address, unicast.OnLinkPrefixLength));
        }
        node = unicast.Next;
    }
    addresses
}

fn dns_servers(entry: &IP_ADAPTER_ADDRESSES_LH) -> Vec<IpAddr> {
    let mut servers = Vec::new();
    let mut node = entry.FirstDnsServerAddress;
    while !node.is_null() {
        // SAFETY: the chain is owned by the adapter buffer.
        let server = unsafe { &*node };
        if let Some(address) = socket_address(&server.Address) {
            servers.push(address);
        }
        node = server.Next;
    }
    servers
}

/// Decode a `SOCKET_ADDRESS` into an [`IpAddr`].
pub fn socket_address(address: &SOCKET_ADDRESS) -> Option<IpAddr> {
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6, SOCKADDR_IN, SOCKADDR_IN6};

    // SAFETY: the family tag tells us which union arm is live.
    unsafe {
        match address.lpSockaddr.as_ref()?.sa_family {
            AF_INET => {
                let raw = address.lpSockaddr.cast::<SOCKADDR_IN>();
                Some(Ipv4Addr::from((*raw).sin_addr.S_un.S_addr.to_ne_bytes()).into())
            }
            AF_INET6 => {
                let raw = address.lpSockaddr.cast::<SOCKADDR_IN6>();
                Some(Ipv6Addr::from((*raw).sin6_addr.u.Byte).into())
            }
            _ => None,
        }
    }
}

fn format_mac(bytes: &[u8]) -> Option<String> {
    match bytes.len() {
        0 => None,
        6 => Some(
            bytes
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(":"),
        ),
        // Windows also reports 8 byte addresses on InfiniBand.
        len if len % 2 == 0 => Some(
            bytes
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(":"),
        ),
        _ => None,
    }
}

fn wide_to_string(ptr: *const u16) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: the API returns NUL terminated UTF-16 strings.
    let mut length = 0usize;
    unsafe {
        while *ptr.add(length) != 0 {
            length += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(ptr, length))
    }
}

fn cstr_to_string(ptr: *const u8) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: adapter names are NUL terminated bytes.
    unsafe { std::ffi::CStr::from_ptr(ptr.cast()) }
        .to_string_lossy()
        .into_owned()
}

/// Parse `route print` into the unified model.
///
/// ```text
/// Active Routes:
/// Network Destination        Netmask          Gateway       Interface  Metric
///           0.0.0.0          0.0.0.0     192.168.1.1     192.168.1.10     25
/// ```
pub fn parse_route_print(raw: &str) -> Vec<RouteInfo> {
    let mut routes = Vec::new();
    let mut in_table = false;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("Active Routes") {
            in_table = true;
            continue;
        }
        if trimmed.starts_with("Persistent Routes") {
            in_table = true;
            continue;
        }
        if trimmed.starts_with("Network Destination") {
            continue;
        }
        if !in_table {
            continue;
        }

        let cols: Vec<&str> = trimmed.split_whitespace().collect();
        if cols.len() < 5 {
            in_table = false;
            continue;
        }
        let (Ok(destination), Ok(mask), Ok(metric)) = (
            cols[0].parse::<Ipv4Addr>(),
            cols[1].parse::<Ipv4Addr>(),
            cols[4].parse::<u64>(),
        ) else {
            in_table = false;
            continue;
        };
        // A default route legitimately carries a 0.0.0.0 netmask; only a
        // zero mask on a non-zero destination means the table has ended.
        if mask.is_unspecified() && destination != Ipv4Addr::UNSPECIFIED {
            in_table = false;
            continue;
        }

        routes.push(RouteInfo {
            destination: (destination != Ipv4Addr::UNSPECIFIED)
                .then(|| format!("{destination}/{}", prefix_length(mask))),
            gateway: cols[2].parse::<Ipv4Addr>().ok().map(IpAddr::V4),
            interface: Some(cols[3].to_string()),
            metric: Some(metric),
        });
    }
    routes
}

fn prefix_length(mask: Ipv4Addr) -> u8 {
    mask.octets().iter().fold(0u8, |acc, byte| {
        acc + (0..8).filter(|bit| byte & (0x80 >> bit) != 0).count() as u8
    })
}

/// Trait object helper.
pub fn manager() -> std::sync::Arc<dyn NetworkManager> {
    std::sync::Arc::new(WindowsNetwork::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str =
        "===========================================================================\n\
        Interface List\n\
        12...00 15 5d 3a 4b 21 ......Hyper-V Virtual Ethernet Adapter\n\
        ===========================================================================\n\
        IPv4 Route Table\n\
        ============================================================================\n\
        Active Routes:\n\
        Network Destination        Netmask          Gateway       Interface  Metric\n\
                  0.0.0.0          0.0.0.0     192.168.1.1     192.168.1.10     25\n\
            127.0.0.0        255.0.0.0         On-link         127.0.0.1    331\n";

    #[test]
    fn route_print_parsing_keeps_the_default_route() {
        let routes = parse_route_print(SAMPLE);
        assert_eq!(routes.len(), 2);

        let default = &routes[0];
        assert!(default.destination.is_none(), "{default:?}");
        assert_eq!(
            default.gateway,
            Some("192.168.1.1".parse::<IpAddr>().unwrap())
        );
        assert_eq!(default.interface.as_deref(), Some("192.168.1.10"));
        assert_eq!(default.metric, Some(25));
        assert!(x_core::network::is_default_route(default));

        assert_eq!(routes[1].destination.as_deref(), Some("127.0.0.0/8"));
        assert_eq!(routes[1].gateway, None, "on-link routes have no gateway");
    }

    #[test]
    fn mac_addresses_are_formatted_or_dropped() {
        assert_eq!(
            format_mac(&[0, 17, 34, 51, 68, 85]).as_deref(),
            Some("00:11:22:33:44:55")
        );
        assert_eq!(format_mac(&[]), None);
        assert_eq!(format_mac(&[1, 2, 3]), None);
    }

    #[test]
    fn adapters_are_enumerable_and_carry_a_loopback() {
        let adapters = adapters().expect("adapters");
        assert!(!adapters.is_empty());
        assert!(
            adapters
                .iter()
                .any(|a| a.friendly_name.to_lowercase().contains("loopback")),
            "no loopback adapter in {adapters:?}"
        );
    }

    #[test]
    fn dns_servers_are_reported() {
        let config = WindowsNetwork::new().dns().expect("dns");
        assert!(!config.servers.is_empty());
    }
}
