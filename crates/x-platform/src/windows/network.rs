//! Windows network adapter: `GetAdaptersAddresses` for interfaces, addresses
//! and DNS servers, `GetIpForwardTable2` for the routing table, and the IP
//! helper ICMP API for reachability probes (see [`super::netprobe`]).

use super::buffer::AlignedBuffer;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use windows_sys::Win32::Foundation::NO_ERROR;
use windows_sys::Win32::NetworkManagement::IpHelper::{
    FreeMibTable, GetAdaptersAddresses, GetIfEntry2, GetIpForwardTable2, GAA_FLAG_INCLUDE_GATEWAYS,
    GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_MULTICAST, IP_ADAPTER_ADDRESSES_LH,
    IP_ADAPTER_GATEWAY_ADDRESS_LH, MIB_IF_ROW2, MIB_IPFORWARD_ROW2, MIB_IPFORWARD_TABLE2,
};
use windows_sys::Win32::Networking::WinSock::{SOCKADDR_INET, SOCKET_ADDRESS};
use x_core::error::{Error, Result};
use x_core::network::{
    AddressInfo, DnsConfig, DnsServer, InterfaceInfo, InterfaceState, NetworkManager, RouteInfo,
};

const AF_UNSPEC: u32 = 0;
const IF_OPER_STATUS_UP: i32 = 1;
/// `IP_ADAPTER_ADDRESSES.Flags`, bit 0: the adapter got its address from DHCP.
const FLAG_DHCP_ENABLED: u32 = 0x0000_0001;
/// `MIB_IPFORWARD_ROW2.NextHop` is a `SOCKADDR_INET` union; only the family
/// tag decides which arm is live.
const AF_INET_U16: u16 = windows_sys::Win32::Networking::WinSock::AF_INET;
const AF_INET6_U16: u16 = windows_sys::Win32::Networking::WinSock::AF_INET6;

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
    /// Link speed in bit/s as reported by the driver, `0` when unknown.
    pub link_speed: u64,
    /// DHCP really supplied the current address.
    pub dhcp_enabled: bool,
    /// IPv4 interface index, used to join routing table rows to names.
    pub if_index: u32,
    /// Next-hop addresses the adapter was configured with.
    pub gateways: Vec<IpAddr>,
}

impl Adapter {
    /// `u64::MAX` and `u64::MAX - 1` are the driver's "no media / speed not
    /// negotiated" sentinels, not link speeds (observed on disconnected
    /// Ethernet and Wi-Fi Direct virtual adapters).
    fn known_link_speed(bits_per_second: u64) -> Option<u64> {
        if bits_per_second == 0 || bits_per_second >= u64::MAX - 1 {
            return None;
        }
        Some(bits_per_second)
    }

    /// Translate into the unified interface model.
    pub fn interface(&self) -> InterfaceInfo {
        let traffic = if_index_traffic(self.if_index);
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
            link_speed_bps: Self::known_link_speed(self.link_speed),
            received_bytes: traffic.0,
            transmitted_bytes: traffic.1,
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
                // The DHCP flag describes how the *address* was obtained.
                // IPv6 link-local addresses come from SLAAC, IPv4 link-local
                // ones from the autoconfiguration fallback when DHCP did not
                // answer, and an adapter that is down holds a stale lease at
                // best; none of those deserve a `yes`.
                dhcp: match *address {
                    IpAddr::V4(v4)
                        if !v4.is_link_local() && self.oper_status == IF_OPER_STATUS_UP =>
                    {
                        Some(self.dhcp_enabled)
                    }
                    _ => None,
                },
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
        // `GetIpForwardTable2` is the native route enumeration; the `route`
        // command it replaced writes localized table headers, which no parser
        // can survive across locales.
        let names: std::collections::HashMap<u32, String> = adapters()
            .unwrap_or_default()
            .into_iter()
            .map(|adapter| (adapter.if_index, adapter.friendly_name))
            .collect();

        let mut routes = forward_table(AF_INET_U16, &names)?;
        routes.extend(forward_table(AF_INET6_U16, &names)?);
        if routes.is_empty() {
            return Err(Error::system(
                "the IP helper API returned an empty routing table",
            ));
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

    fn resolve(&self, host: &str) -> Result<Vec<IpAddr>> {
        super::netprobe::resolve(host)
    }

    fn reverse_dns(&self, address: IpAddr) -> Result<String> {
        super::netprobe::reverse_dns(address)
    }

    fn flush_dns_cache(&self) -> Result<()> {
        super::netprobe::flush_dns_cache()
    }

    fn ping(&self, request: &x_core::network::PingRequest) -> Result<x_core::network::PingSummary> {
        super::netprobe::ping(request)
    }

    fn trace(
        &self,
        address: IpAddr,
        max_hops: u32,
        timeout_ms: u32,
    ) -> Result<Vec<x_core::network::TraceHop>> {
        super::netprobe::trace(address, max_hops, timeout_ms)
    }

    fn tls_info(&self, host: &str, port: u16, timeout_ms: u64) -> Result<x_core::netdiag::TlsInfo> {
        crate::common::netdiag::tls_info(host, port, timeout_ms)
    }

    fn http_probe(
        &self,
        url: &str,
        method: &str,
        timeout_ms: u64,
    ) -> Result<x_core::netdiag::HttpResponse> {
        crate::common::netdiag::http_probe(url, method, timeout_ms)
    }
}

/// Cumulative byte counters for one interface, from `GetIfEntry2`.
///
/// `GetAdaptersAddresses` does not carry traffic; the MIB row keyed by the
/// interface index is the native source. Interfaces that vanished between
/// enumeration and the query answer with an error, which maps to `None`.
fn if_index_traffic(if_index: u32) -> (Option<u64>, Option<u64>) {
    // `windows-sys` does not project the anonymous first member holding
    // `SizeOfRow`, only the `NET_LUID` sharing that slot: leave it zero.
    // Live `GetIfEntry2` accepts a zeroed LUID with a real index, while
    // writing the struct size there (which the docs suggest) turns the LUID
    // into a bogus lookup key and fails.
    let mut row = MIB_IF_ROW2 {
        InterfaceIndex: if_index,
        ..Default::default()
    };
    // SAFETY: `row` is a writable, fully zeroed `MIB_IF_ROW2` apart from the
    // index; the call only fills it in when it succeeds.
    if unsafe { GetIfEntry2(&mut row) } == NO_ERROR {
        (Some(row.InOctets), Some(row.OutOctets))
    } else {
        (None, None)
    }
}

/// Enumerate every adapter with its addresses and DNS servers.
pub fn adapters() -> Result<Vec<Adapter>> {
    // DNS servers and gateway addresses are part of the answer, so
    // `GAA_FLAG_SKIP_DNS_SERVER` must not be set and
    // `GAA_FLAG_INCLUDE_GATEWAYS` must be; the flags only drop the address
    // families we never render.
    let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_INCLUDE_GATEWAYS;
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
        // `IfIndex` and `Flags` live in leading unions of the versioned struct.
        // SAFETY: both union arms are the raw `u32` the API wrote.
        let (if_index, flags) =
            unsafe { (entry.Anonymous1.Anonymous.IfIndex, entry.Anonymous2.Flags) };
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
            // The driver reports the same figure for both directions on every
            // NIC we have seen; the receive side is the one `msinfo32` shows.
            link_speed: entry.ReceiveLinkSpeed,
            dhcp_enabled: flags & FLAG_DHCP_ENABLED != 0,
            if_index,
            gateways: gateway_addresses(entry),
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

fn gateway_addresses(entry: &IP_ADAPTER_ADDRESSES_LH) -> Vec<IpAddr> {
    let mut gateways = Vec::new();
    let mut node = entry.FirstGatewayAddress;
    while !node.is_null() {
        // SAFETY: the chain is owned by the adapter buffer.
        let gateway = unsafe { &*(node as *const IP_ADAPTER_GATEWAY_ADDRESS_LH) };
        if let Some(address) = socket_address(&gateway.Address) {
            gateways.push(address);
        }
        node = gateway.Next;
    }
    gateways
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

/// Read one address family of the routing table through `GetIpForwardTable2`.
///
/// The API hands back a single allocation whose rows point at prefix and
/// next-hop unions; interface indexes are translated to adapter display names
/// so the rows join with `x net interfaces` on the same string.
fn forward_table(
    family: windows_sys::Win32::Networking::WinSock::ADDRESS_FAMILY,
    names: &std::collections::HashMap<u32, String>,
) -> Result<Vec<RouteInfo>> {
    let mut table: *mut MIB_IPFORWARD_TABLE2 = std::ptr::null_mut();
    // SAFETY: the API allocates and writes the table pointer we supply.
    let rc = unsafe { GetIpForwardTable2(family, &mut table) };
    if rc != 0 || table.is_null() {
        return Ok(Vec::new());
    }
    // SAFETY: rows are indexed within `NumEntries` while the pointer is live,
    // and `FreeMibTable` runs after the last read.
    let routes = unsafe {
        let header = &*table;
        // `Table` is a one-element placeholder for a variable length array, so
        // the rows must be reached through a raw slice of the real count.
        let rows = std::slice::from_raw_parts(
            std::ptr::addr_of!((*table).Table) as *const MIB_IPFORWARD_ROW2,
            header.NumEntries as usize,
        );
        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let destination = inet_address(&row.DestinationPrefix.Prefix);
            let prefix_len = row.DestinationPrefix.PrefixLength;
            let unspecified = destination.is_none_or(|address| match address {
                IpAddr::V4(ip) => ip.is_unspecified(),
                IpAddr::V6(ip) => ip.is_unspecified(),
            });
            let gateway = inet_address(&row.NextHop).filter(|address| match address {
                IpAddr::V4(ip) => !ip.is_unspecified(),
                IpAddr::V6(ip) => !ip.is_unspecified(),
            });
            out.push(RouteInfo {
                destination: (!unspecified)
                    .then(|| format!("{}", destination.expect("checked above")))
                    .map(|text| format!("{text}/{prefix_len}")),
                gateway,
                interface: names
                    .get(&row.InterfaceIndex)
                    .cloned()
                    .or_else(|| Some(row.InterfaceIndex.to_string())),
                metric: (row.Metric > 0 && row.Metric != u32::MAX).then_some(row.Metric as u64),
            });
        }
        out
    };
    // SAFETY: the table came from GetIpForwardTable2 and is freed exactly once.
    unsafe { FreeMibTable(table.cast()) };
    Ok(routes)
}

fn inet_address(inet: &SOCKADDR_INET) -> Option<IpAddr> {
    // SAFETY: the family tag selects the live union arm.
    unsafe {
        match inet.si_family {
            AF_INET_U16 => {
                Some(Ipv4Addr::from(inet.Ipv4.sin_addr.S_un.S_addr.to_ne_bytes()).into())
            }
            AF_INET6_U16 => Some(Ipv6Addr::from(inet.Ipv6.sin6_addr.u.Byte).into()),
            _ => None,
        }
    }
}

/// Trait object helper.
pub fn manager() -> std::sync::Arc<dyn NetworkManager> {
    std::sync::Arc::new(WindowsNetwork::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn driver_link_speed_sentinels_are_not_speeds() {
        assert_eq!(Adapter::known_link_speed(0), None);
        assert_eq!(Adapter::known_link_speed(u64::MAX), None);
        assert_eq!(Adapter::known_link_speed(u64::MAX - 1), None);
        assert_eq!(
            Adapter::known_link_speed(1_000_000_000),
            Some(1_000_000_000)
        );
    }

    #[test]
    fn dhcp_flag_only_labels_live_routable_ipv4_addresses() {
        let adapter = Adapter {
            oper_status: IF_OPER_STATUS_UP,
            dhcp_enabled: true,
            addresses: vec![
                ("192.168.3.64".parse().unwrap(), 24),
                ("169.254.1.2".parse().unwrap(), 16),
                ("fe80::1".parse().unwrap(), 64),
            ],
            ..Adapter::default()
        };
        let rows = adapter.address_rows();
        assert_eq!(rows[0].dhcp, Some(true), "routable IPv4 from DHCP");
        assert_eq!(rows[1].dhcp, None, "APIPA is not DHCP");
        assert_eq!(rows[2].dhcp, None, "SLAAC is not DHCP");
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
    fn interfaces_carry_traffic_counters() {
        let rows = WindowsNetwork::new().interfaces().expect("interfaces");
        assert!(
            rows.iter().any(|r| r.received_bytes.is_some()),
            "GetIfEntry2 must answer for at least one interface: {rows:?}"
        );
    }

    #[test]
    fn dns_servers_are_reported() {
        let config = WindowsNetwork::new().dns().expect("dns");
        assert!(!config.servers.is_empty());
    }
}
