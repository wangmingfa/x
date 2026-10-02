//! Shared `getifaddrs` traversal for the BSD-derived platforms.
//!
//! macOS and Linux expose the same interface enumeration API, so the walk, the
//! address decoding and the [`InterfaceInfo`]/[`AddressInfo`] mapping are written
//! once here. Only three things differ between the two and they are isolated
//! below: the `sockaddr` prefix, the link level address layout, and where the
//! interface MTU comes from.

use std::collections::BTreeMap;
#[cfg(target_os = "linux")]
use std::collections::BTreeSet;
use std::ffi::CStr;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::os::raw::{c_char, c_int};
use x_core::error::{Error, Result};
use x_core::network::{AddressInfo, InterfaceInfo, InterfaceState};

const AF_INET: u16 = 2;
const AF_INET6: u16 = 10;
const IFF_UP: c_int = 0x1;

#[cfg(target_os = "macos")]
mod raw {
    use std::os::raw::{c_char, c_int, c_void};

    pub const AF_LINK: u16 = 18;

    #[repr(C)]
    pub struct ifaddrs {
        pub ifa_next: *mut ifaddrs,
        pub ifa_name: *mut c_char,
        pub ifa_flags: c_int,
        pub ifa_addr: *mut sockaddr,
        pub ifa_netmask: *mut sockaddr,
        pub ifa_ifu: *mut sockaddr,
        pub ifa_data: *mut c_void,
    }

    #[repr(C)]
    pub struct sockaddr {
        pub sa_len: u8,
        pub sa_family: u8,
        pub sa_data: [i8; 14],
    }

    #[repr(C)]
    pub struct sockaddr_in {
        pub sin_len: u8,
        pub sin_family: u8,
        pub sin_port: u16,
        pub sin_addr: [u8; 4],
        pub sin_zero: [u8; 8],
    }

    #[repr(C)]
    pub struct sockaddr_in6 {
        pub sin6_len: u8,
        pub sin6_family: u8,
        pub sin6_port: u16,
        pub sin6_flowinfo: u32,
        pub sin6_addr: [u8; 16],
        pub sin6_scope_id: u32,
    }

    #[repr(C)]
    pub struct sockaddr_dl {
        pub sdl_len: u8,
        pub sdl_family: u8,
        pub sdl_index: u16,
        pub sdl_type: u8,
        pub sdl_nlen: u8,
        pub sdl_alen: u8,
        pub sdl_slen: u8,
        pub sdl_data: [c_char; 12],
    }

    pub const IFF_RUNNING: c_int = 0x40;
    pub const LINKTYPE_ETHER: u8 = 6;
    pub const LINKTYPE_LOOPBACK: u8 = 0;
}

#[cfg(target_os = "linux")]
mod raw {
    use std::os::raw::{c_char, c_int, c_void};

    pub const AF_LINK: u16 = 17;

    #[repr(C)]
    pub struct ifaddrs {
        pub ifa_next: *mut ifaddrs,
        pub ifa_name: *mut c_char,
        pub ifa_flags: c_int,
        pub ifa_addr: *mut sockaddr,
        pub ifa_netmask: *mut sockaddr,
        pub ifa_ifu: *mut sockaddr,
        pub ifa_data: *mut c_void,
    }

    #[repr(C)]
    pub struct sockaddr {
        pub sa_family: u16,
        pub sa_data: [i8; 14],
    }

    #[repr(C)]
    pub struct sockaddr_in {
        pub sin_family: u16,
        pub sin_port: u16,
        pub sin_addr: [u8; 4],
        pub sin_zero: [u8; 8],
    }

    #[repr(C)]
    pub struct sockaddr_in6 {
        pub sin6_family: u16,
        pub sin6_port: u16,
        pub sin6_flowinfo: u32,
        pub sin6_addr: [u8; 16],
        pub sin6_scope_id: u32,
    }

    #[repr(C)]
    pub struct sockaddr_ll {
        pub sll_family: u16,
        pub sll_protocol: u16,
        pub sll_ifindex: c_int,
        pub sll_hatype: u16,
        pub sll_pkttype: u8,
        pub sll_halen: u8,
        pub sll_addr: [u8; 8],
    }

    pub const IFF_RUNNING: c_int = 0x40;
    pub const ARPHRD_ETHER: u16 = 1;
    pub const ARPHRD_LOOPBACK: u16 = 772;
}

use raw::*;

extern "C" {
    fn getifaddrs(ifap: *mut *mut ifaddrs) -> c_int;
    fn freeifaddrs(ifa: *mut ifaddrs);
}

struct IfAddrsGuard(*mut ifaddrs);

impl Drop for IfAddrsGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the pointer came from `getifaddrs` and is freed exactly once.
            unsafe { freeifaddrs(self.0) };
        }
    }
}

struct IfAddrs {
    head: *mut ifaddrs,
}

impl IfAddrs {
    fn new() -> Result<Self> {
        let mut head: *mut ifaddrs = std::ptr::null_mut();
        // SAFETY: `getifaddrs` writes the owned head pointer we pass in.
        let rc = unsafe { getifaddrs(&mut head) };
        if rc != 0 || head.is_null() {
            return Err(Error::system(format!("getifaddrs failed with status {rc}")));
        }
        Ok(Self { head })
    }

    /// Every entry, in kernel order.
    fn entries(&self) -> Vec<Entry> {
        let mut entries = Vec::new();
        let mut node = self.head;
        while !node.is_null() {
            // SAFETY: the list is owned by `self` and freed in `drop`.
            let entry = unsafe { &*node };
            entries.push(Entry {
                name: cstr_to_string(entry.ifa_name),
                flags: entry.ifa_flags,
                family: family_of(entry.ifa_addr),
                address: entry.ifa_addr,
                netmask: entry.ifa_netmask,
            });
            node = entry.ifa_next;
        }
        entries
    }
}

/// A borrowed, owned-name view of one `ifaddrs` node.
struct Entry {
    name: String,
    flags: c_int,
    family: u16,
    address: *mut sockaddr,
    netmask: *mut sockaddr,
}

/// Enumerate interfaces with their state and link level address.
pub fn interfaces() -> Result<Vec<InterfaceInfo>> {
    let list = IfAddrs::new()?;
    let _guard = IfAddrsGuard(list.head);

    // One `ifaddrs` chain lists the same interface once per address family plus
    // one link level entry, so rows are merged by name instead of being emitted
    // per entry.
    let traffic = traffic_table();
    let mut rows: BTreeMap<String, InterfaceInfo> = BTreeMap::new();
    for entry in list.entries() {
        if entry.name.is_empty() {
            continue;
        }
        let up = entry.flags & IFF_UP != 0 && entry.flags & IFF_RUNNING != 0;
        // A link level entry carries the hardware address; the address entries of
        // the same interface only add the IP view.
        let (mac, description) = if entry.family == AF_LINK {
            let (mac, kind) = link_address(entry.address);
            (mac, Some(kind))
        } else {
            (None, None)
        };

        rows.entry(entry.name.clone())
            .and_modify(|row| {
                // The most informative entry wins: any family reporting the
                // interface as up is enough to call it up.
                if up {
                    row.state = InterfaceState::Up;
                }
                if row.mac_address.is_none() {
                    row.mac_address = mac.clone();
                    row.description = description.clone();
                }
            })
            .or_insert_with(|| {
                let (received_bytes, transmitted_bytes) = traffic
                    .get(entry.name.as_str())
                    .copied()
                    .unwrap_or((None, None));
                InterfaceInfo {
                    name: entry.name.clone(),
                    description,
                    mac_address: mac,
                    state: if up {
                        InterfaceState::Up
                    } else {
                        InterfaceState::Down
                    },
                    mtu: mtu(&entry.name),
                    link_speed_bps: speed(&entry.name),
                    received_bytes,
                    transmitted_bytes,
                }
            });
    }

    Ok(rows.into_values().collect())
}

/// Per-interface byte counters since boot.
///
/// sysfs keeps a file per counter under `/sys/class/net`; interfaces that do
/// not appear here (deleted between enumeration and read) simply show `None`.
#[cfg(target_os = "linux")]
fn traffic_table() -> BTreeMap<String, (Option<u64>, Option<u64>)> {
    let mut table = BTreeMap::new();
    let Ok(links) = std::fs::read_dir("/sys/class/net") else {
        return table;
    };
    for link in links.flatten() {
        let name = link.file_name().to_string_lossy().into_owned();
        let counter = |file: &str| {
            std::fs::read_to_string(link.path().join("statistics").join(file))
                .ok()
                .and_then(|raw| raw.trim().parse::<u64>().ok())
        };
        table.insert(name, (counter("rx_bytes"), counter("tx_bytes")));
    }
    table
}

/// Per-interface byte counters since boot.
///
/// Reading `if_data` out of `getifaddrs` would mean hard-coding the struct
/// field offsets, which differ per macOS release, so the counters come from
/// `netstat -ib` with columns located by their header positions: BSD netstat
/// right-aligns every value inside its column, so the token starting at (or
/// after) a header token always belongs to that column. This is a level-3
/// source but not a text locale risk: the header tokens are fixed English.
#[cfg(target_os = "macos")]
fn traffic_table() -> BTreeMap<String, (Option<u64>, Option<u64>)> {
    let raw = crate::sys::run_command("netstat", &["-ib"]).unwrap_or_default();
    parse_netstat_ib(&raw)
}

/// Parse `netstat -ib` output, keeping one row per interface (the `<Link#N>`
/// line, which carries the interface-wide counters rather than per-address ones).
#[cfg(target_os = "macos")]
fn parse_netstat_ib(raw: &str) -> BTreeMap<String, (Option<u64>, Option<u64>)> {
    let mut table = BTreeMap::new();
    let Some(header) = raw
        .lines()
        .find(|line| line.contains("Ibytes") && line.contains("Obytes"))
    else {
        return table;
    };
    let column = |token: &str| header.find(token);
    let (Some(name_col), Some(network_col), Some(ibytes_col), Some(obytes_col)) = (
        column("Name"),
        column("Network"),
        column("Ibytes"),
        column("Obytes"),
    ) else {
        return table;
    };
    let field = |line: &str, start: usize| {
        line.get(start..)?
            .split_whitespace()
            .next()
            .and_then(|value| value.parse::<u64>().ok())
    };

    for line in raw.lines().skip_while(|line| *line != header).skip(1) {
        let Some(name) = line
            .get(name_col..)
            .and_then(|rest| rest.split_whitespace().next())
        else {
            continue;
        };
        // Continuation rows (IPv6 address lines) have no name and no counters.
        if name.is_empty() || table.contains_key(name) {
            continue;
        }
        let is_link_row = line
            .get(network_col..)
            .and_then(|rest| rest.split_whitespace().next())
            .is_some_and(|network| network.starts_with("<Link"));
        if !is_link_row {
            continue;
        }
        table.insert(
            name.to_string(),
            (field(line, ibytes_col), field(line, obytes_col)),
        );
    }
    table
}

/// Enumerate configured IP addresses with their prefix lengths.
pub fn addresses() -> Result<Vec<AddressInfo>> {
    let list = IfAddrs::new()?;
    let _guard = IfAddrsGuard(list.head);

    // Which interfaces are addressed dynamically, gathered once rather than per
    // address. The kernel does not record how an address was configured, so
    // this asks the tools that do know.
    #[cfg(target_os = "linux")]
    let dynamic = linux_dynamic_interfaces();

    let mut rows = Vec::new();
    for entry in list.entries() {
        if entry.name.is_empty() {
            continue;
        }
        let Some((address, prefix_len)) =
            decode_address(entry.address, entry.netmask, entry.family)
        else {
            continue;
        };
        #[cfg(target_os = "linux")]
        // Only IPv4 is labelled: an IPv6 address marked `dynamic` came from
        // SLAAC or DHCPv6, and a DHCPv4 lease says nothing about which.
        let dhcp = if address.is_ipv4() {
            dynamic.contains(&entry.name).then_some(true)
        } else {
            None
        };
        #[cfg(not(target_os = "linux"))]
        let dhcp = None;
        rows.push(AddressInfo {
            interface: entry.name,
            address,
            prefix_len,
            dhcp,
        });
    }

    rows.sort_by(|a, b| {
        a.interface
            .cmp(&b.interface)
            .then_with(|| a.address.to_string().cmp(&b.address.to_string()))
    });
    Ok(rows)
}

/// The family tag of a `sockaddr`, or `0` when there is no address.
fn family_of(address: *mut sockaddr) -> u16 {
    if address.is_null() {
        return 0;
    }
    // `family_raw` is the only place that dereferences the pointer, and it is
    // `unsafe` internally.
    family_raw(address)
}

#[cfg(target_os = "macos")]
fn family_raw(address: *mut sockaddr) -> u16 {
    unsafe { (*address).sa_family as u16 }
}

#[cfg(target_os = "linux")]
fn family_raw(address: *mut sockaddr) -> u16 {
    unsafe { (*address).sa_family }
}

/// Interfaces whose IPv4 addresses were assigned dynamically.
///
/// The kernel does not record how an address came to be, so this asks the two
/// tools that do, and takes whichever answers:
///
/// - `ip -4 addr show` tags a DHCP-assigned address `dynamic`. This is the
///   kernel's own view and needs no daemon.
/// - `nmcli -g IP4.METHOD,DEVICE con show --active` reports `auto` per
///   connection, which covers NetworkManager configurations where the address
///   already looks static once installed.
///
/// Only a positive answer is recorded: an interface missing from both lists is
/// left out entirely rather than reported as `false`, because "no tool said"
/// is not "statically configured" — a machine running neither, or a third-party
/// DHCP client, would otherwise be mislabelled.
#[cfg(target_os = "linux")]
fn linux_dynamic_interfaces() -> BTreeSet<String> {
    let mut out = BTreeSet::new();

    if let Ok(text) = crate::sys::run_command("ip", &["-4", "addr", "show"]) {
        collect_ip_dynamic(&text, &mut out);
    }
    if let Ok(text) = crate::sys::run_command(
        "nmcli",
        &["-g", "IP4.METHOD,DEVICE", "connection", "show", "--active"],
    ) {
        collect_nmcli_dynamic(&text, &mut out);
    }
    out
}

/// The interface name from an `ip addr show` block header.
///
/// The header is `3: enp0s3: <BROADCAST,...>`: the index comes first, then the
/// name, and taking everything after the *first* colon would swallow the flags
/// along with it. A virtual link prints `eth0@if42:`, where the `@if42` suffix
/// is the peer index and not part of the name.
#[cfg(target_os = "linux")]
fn header_interface(line: &str) -> String {
    let Some((_, rest)) = line.split_once(':') else {
        return String::new();
    };
    let Some((name, _)) = rest.split_once(':') else {
        return String::new();
    };
    name.split('@').next().unwrap_or(name).trim().to_string()
}

/// `ip addr show` output, collected into the interfaces owning a `dynamic`
/// address.
///
/// The block looks like:
///
/// ```text
/// 3: enp0s3: <BROADCAST,...> mtu 1500 ...
///     inet 192.168.1.23/24 brd ... scope global dynamic enp0s3
/// ```
///
/// so the interface is the `N: name:` header and each `scope global dynamic`
/// line marks that interface as DHCP-addressed.
#[cfg(target_os = "linux")]
fn collect_ip_dynamic(text: &str, out: &mut BTreeSet<String>) {
    let mut current = String::new();
    for line in text.lines() {
        if !line.starts_with(char::is_whitespace) {
            // A new block header: `3: eth0@if2: <BROADCAST,...>`.
            current = header_interface(line);
            continue;
        }
        let trimmed = line.trim();
        if !trimmed.starts_with("inet ") {
            continue;
        }
        // Only `scope global` matters: link-local DHCP addresses are not
        // routable configuration, and loopback never carries the tag.
        if trimmed.contains("scope global") && trimmed.contains(" dynamic") && !current.is_empty() {
            out.insert(current.clone());
        }
    }
}

/// `nmcli -g IP4.METHOD,DEVICE connection show --active` output.
///
/// One `method:device` pair per line, where `auto` is NetworkManager's word
/// for DHCP and anything else (`manual`, `disabled`, `link-local`, …) is not.
#[cfg(target_os = "linux")]
fn collect_nmcli_dynamic(text: &str, out: &mut BTreeSet<String>) {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // The `-g` (terse) output is `method:device`.
        let Some((method, device)) = line.split_once(':') else {
            continue;
        };
        if method.trim() != "auto" {
            continue;
        }
        // The device column may itself be a colon-separated list.
        for name in device.split(':') {
            let name = name.trim();
            if !name.is_empty() {
                out.insert(name.to_string());
            }
        }
    }
}

/// Decode an IPv4 or IPv6 address with the prefix length from its netmask.
///
/// The prefix comes from `ifa_netmask`, never from the address itself: counting
/// the bits set in `192.168.1.23` would report a /32 for every IPv4 address.
fn decode_address(
    address: *mut sockaddr,
    netmask: *mut sockaddr,
    family: u16,
) -> Option<(IpAddr, Option<u8>)> {
    if address.is_null() {
        return None;
    }
    match family {
        AF_INET => {
            // SAFETY: family AF_INET means the payload is a `sockaddr_in`.
            let in4 = unsafe { &*(address as *const sockaddr_in) };
            let address = IpAddr::V4(Ipv4Addr::from(in4.sin_addr));
            let prefix = ipv4_prefix(netmask);
            Some((address, prefix))
        }
        AF_INET6 => {
            // SAFETY: family AF_INET6 means the payload is a `sockaddr_in6`.
            let in6 = unsafe { &*(address as *const sockaddr_in6) };
            let address = IpAddr::V6(Ipv6Addr::from(in6.sin6_addr));
            let prefix = ipv6_prefix(netmask);
            Some((address, prefix))
        }
        _ => None,
    }
}

/// Prefix length of an IPv4 netmask, or `None` when the kernel did not report one.
fn ipv4_prefix(netmask: *mut sockaddr) -> Option<u8> {
    if netmask.is_null() || family_of(netmask) != AF_INET {
        return None;
    }
    // SAFETY: the family tag says the payload is a `sockaddr_in`.
    let mask = unsafe { &*(netmask as *const sockaddr_in) };
    let bits = u32::from_be_bytes(mask.sin_addr);
    // A netmask is a contiguous run of set bits followed by zeros, so the prefix
    // is how many bits are set. `/0` (no bits set) is a valid default route.
    let prefix = bits.count_ones();
    let contiguous = prefix == 0 || bits == (!0u32) << (u32::BITS - prefix);
    contiguous.then_some(prefix as u8)
}

/// Prefix length of an IPv6 netmask, or `None` when the kernel did not report one.
fn ipv6_prefix(netmask: *mut sockaddr) -> Option<u8> {
    if netmask.is_null() || family_of(netmask) != AF_INET6 {
        return None;
    }
    // SAFETY: the family tag says the payload is a `sockaddr_in6`.
    let mask = unsafe { &*(netmask as *const sockaddr_in6) };
    let bits = u128::from_be_bytes(mask.sin6_addr);
    let prefix = bits.count_ones();
    let contiguous = prefix == 0 || bits == (!0u128) << (u128::BITS - prefix);
    contiguous.then_some(prefix as u8)
}

fn cstr_to_string(ptr: *const c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: interface names are NUL terminated by the kernel.
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}

/// Hardware address and a short description of the link type.
#[cfg(target_os = "macos")]
fn link_address(address: *mut sockaddr) -> (Option<String>, String) {
    if address.is_null() {
        return (None, "unknown".to_string());
    }
    // SAFETY: an AF_LINK entry carries a `sockaddr_dl`.
    let sdl = unsafe { &*(address as *const sockaddr_dl) };
    let bytes: Vec<u8> = sdl.sdl_data.iter().map(|b| *b as u8).collect();
    let mac = format_mac(&bytes[..sdl.sdl_alen as usize]);
    let kind = if sdl.sdl_type == LINKTYPE_ETHER {
        "ethernet"
    } else if sdl.sdl_type == LINKTYPE_LOOPBACK {
        "loopback"
    } else {
        return (mac, format!("link type {}", sdl.sdl_type));
    };
    (mac, kind.to_string())
}

/// Hardware address and a short description of the link type.
#[cfg(target_os = "linux")]
fn link_address(address: *mut sockaddr) -> (Option<String>, String) {
    if address.is_null() {
        return (None, "unknown".to_string());
    }
    // SAFETY: an AF_PACKET entry carries a `sockaddr_ll`.
    let sll = unsafe { &*(address as *const sockaddr_ll) };
    let mac = format_mac(&sll.sll_addr[..sll.sll_halen as usize]);
    let kind = if sll.sll_hatype == ARPHRD_ETHER {
        "ethernet"
    } else if sll.sll_hatype == ARPHRD_LOOPBACK {
        "loopback"
    } else {
        return (mac, format!("link type {}", sll.sll_hatype));
    };
    (mac, kind.to_string())
}

fn format_mac(bytes: &[u8]) -> Option<String> {
    match bytes.len() {
        0 => None,
        4 | 6 | 8 => Some(
            bytes
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(":"),
        ),
        _ => None,
    }
}

/// Interface MTU, used for the interface list only.
///
/// macOS has no `net.interface.<if>.mtu` sysctl, so the value comes from the
/// `SIOCGIFMTU` ioctl on a datagram socket.
#[cfg(target_os = "macos")]
pub fn mtu(name: &str) -> Option<u32> {
    let name = name.as_bytes();
    if name.is_empty() || name.len() >= libc::IFNAMSIZ {
        return None;
    }
    // SAFETY: a zeroed `ifreq` is a valid request buffer; only `ifr_name` is
    // filled in by us and the kernel only writes the MTU back.
    let mut request: libc::ifreq = unsafe { std::mem::zeroed() };
    for (slot, byte) in request.ifr_name.iter_mut().zip(name) {
        *slot = *byte as libc::c_char;
    }
    // SAFETY: the socket is only used to issue the ioctl below.
    let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0) };
    if fd < 0 {
        return None;
    }
    let rc = unsafe { libc::ioctl(fd, libc::SIOCGIFMTU as libc::c_ulong, &mut request) };
    // SAFETY: `fd` came from `socket` above and is not used again.
    unsafe { libc::close(fd) };
    if rc != 0 {
        return None;
    }
    // SAFETY: a successful `SIOCGIFMTU` fills in `ifr_mtu` of the union.
    let mtu = unsafe { request.ifr_ifru.ifru_mtu };
    (mtu > 0).then_some(mtu as u32)
}

/// Interface MTU, used for the interface list only.
#[cfg(target_os = "linux")]
pub fn mtu(name: &str) -> Option<u32> {
    std::fs::read_to_string(format!("/sys/class/net/{name}/mtu"))
        .ok()
        .and_then(|raw| raw.trim().parse().ok())
}

/// Negotiated link speed in bits per second.
///
/// sysfs exposes the current speed in Mbps; a down or virtual interface answers
/// with an error on the read, which is exactly the "no speed" answer.
#[cfg(target_os = "linux")]
pub fn speed(name: &str) -> Option<u64> {
    std::fs::read_to_string(format!("/sys/class/net/{name}/speed"))
        .ok()
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .filter(|mbps| *mbps > 0)
        .map(|mbps| mbps as u64 * 1_000_000)
}

/// Negotiated link speed in bits per second.
///
/// macOS has no sysctl for it, so the `ifconfig` media line is the level-3
/// source: `media: 1000baseT <full-duplex>` is a gigabit link, while Wi-Fi
/// reports `media: autoselect` and has no conventional link speed to show.
#[cfg(target_os = "macos")]
pub fn speed(name: &str) -> Option<u64> {
    let raw = crate::sys::run_command("ifconfig", &[name]).ok()?;
    parse_ifconfig_speed(&raw)
}

/// Extract the link speed from an `ifconfig` interface dump.
#[cfg(target_os = "macos")]
fn parse_ifconfig_speed(raw: &str) -> Option<u64> {
    let line = raw
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("media:"))?;
    let token = line.split_whitespace().nth(1)?;
    // Media names look like `1000baseT`, `100baseTX`, `10000baseSX`; the
    // leading number is Mbps. `autoselect` and friends carry no number.
    let digits: String = token.chars().take_while(char::is_ascii_digit).collect();
    let mbps: u64 = digits.parse().ok()?;
    (mbps > 0).then(|| mbps * 1_000_000)
}

#[cfg(test)]
mod tests {
    const AF_UNSPEC: u16 = 0;
    use super::*;

    /// Owned IPv4 `sockaddr_in` values, so the pointers outlive the call.
    struct V4 {
        address: Box<raw::sockaddr_in>,
        netmask: Option<Box<raw::sockaddr_in>>,
    }

    impl V4 {
        fn new(address: [u8; 4], netmask: Option<[u8; 4]>) -> Self {
            let build = |addr: [u8; 4]| {
                Box::new(raw::sockaddr_in {
                    #[cfg(target_os = "macos")]
                    sin_len: 16,
                    sin_family: AF_INET as _,
                    sin_port: 0,
                    sin_addr: addr,
                    sin_zero: [0; 8],
                })
            };
            Self {
                address: build(address),
                netmask: netmask.map(build),
            }
        }

        fn address(&mut self) -> *mut sockaddr {
            (&raw mut *self.address).cast()
        }

        fn netmask(&mut self) -> *mut sockaddr {
            self.netmask
                .as_mut()
                .map(|mask| (&raw mut **mask).cast())
                .unwrap_or(std::ptr::null_mut())
        }
    }

    #[test]
    fn the_prefix_comes_from_the_netmask_not_from_the_address() {
        let mut v4 = V4::new([192, 168, 1, 23], Some([255, 255, 255, 0]));
        let (decoded, prefix) =
            decode_address(v4.address(), v4.netmask(), AF_INET).expect("decoded");

        assert_eq!(decoded, "192.168.1.23".parse::<IpAddr>().unwrap());
        assert_eq!(prefix, Some(24));
    }

    #[test]
    fn unusual_prefixes_are_counted_from_the_netmask() {
        for (mask, expected) in [
            ([255, 0, 0, 0], 8u8),
            ([255, 255, 0, 0], 16),
            ([255, 255, 255, 252], 30),
            ([0, 0, 0, 0], 0),
        ] {
            let mut v4 = V4::new([10, 0, 0, 1], Some(mask));
            let (_, prefix) = decode_address(v4.address(), v4.netmask(), AF_INET).expect("decoded");
            assert_eq!(prefix, Some(expected), "mask {mask:?}");
        }
    }

    #[test]
    fn a_missing_netmask_reports_no_prefix_instead_of_a_guess() {
        let mut v4 = V4::new([10, 0, 0, 1], None);
        let (_, prefix) = decode_address(v4.address(), v4.netmask(), AF_INET).expect("decoded");
        assert_eq!(prefix, None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn loopback_reports_its_mtu() {
        assert_eq!(mtu("lo0"), Some(16384));
        assert_eq!(mtu("no-such-interface0"), None);
    }

    #[test]
    fn a_non_contiguous_netmask_is_rejected_instead_of_guessed() {
        let mut v4 = V4::new([10, 0, 0, 1], Some([255, 255, 0, 255]));
        let (_, prefix) = decode_address(v4.address(), v4.netmask(), AF_INET).expect("decoded");

        assert_eq!(prefix, None);
    }

    #[test]
    fn null_addresses_are_skipped() {
        let mut v4 = V4::new([10, 0, 0, 1], Some([255, 0, 0, 0]));
        assert!(decode_address(std::ptr::null_mut(), v4.netmask(), AF_INET).is_none());
        assert!(decode_address(v4.address(), std::ptr::null_mut(), AF_UNSPEC).is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn media_lines_become_link_speeds_or_honest_none() {
        assert_eq!(
            parse_ifconfig_speed("	status: active\n\tmedia: 1000baseT <full-duplex>\n"),
            Some(1_000_000_000)
        );
        assert_eq!(
            parse_ifconfig_speed("\tmedia: 100baseTX <full-duplex>\n"),
            Some(100_000_000)
        );
        // Wi-Fi reports autoselect; there is no conventional link speed to show.
        assert_eq!(parse_ifconfig_speed("\tmedia: autoselect (none)\n"), None);
        assert_eq!(parse_ifconfig_speed("status: active\n"), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn netstat_ib_link_rows_become_traffic_counters() {
        let header =
            "Name  Mtu   Network       Address            Ipkts Ierrs    Ibytes    Opkts Oerrs    Obytes  Coll";
        let network_col = header.find("Network").unwrap();
        let ibytes_col = header.find("Ibytes").unwrap();
        let obytes_col = header.find("Obytes").unwrap();
        let row = |name: &str, network: &str, ibytes: &str, obytes: &str| {
            let mut line = String::new();
            for (column, token) in [
                (0usize, name),
                (network_col, network),
                (ibytes_col, ibytes),
                (obytes_col, obytes),
            ] {
                while line.len() < column {
                    line.push(' ');
                }
                line.push_str(token);
            }
            line
        };
        let raw = format!(
            "{header}\n{}\n{}\n{}\n{}",
            row("lo0", "<Link#1>", "121504031", "121504031"),
            // Per-address continuation rows carry dashes and must not win.
            row("lo0", "127.0.0.1/32", "-", "-"),
            row("en0", "<Link#13>", "7116283124", "91252487"),
            row("en0", "fe80:2::1/64", "-", "-"),
        );

        let table = parse_netstat_ib(&raw);
        assert_eq!(table.get("lo0"), Some(&(Some(121504031), Some(121504031))));
        assert_eq!(table.get("en0"), Some(&(Some(7116283124), Some(91252487))));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn netstat_ib_without_the_expected_header_yields_nothing() {
        assert!(parse_netstat_ib("active internet\nProto Recv-Q").is_empty());
    }

    // --- Linux DHCP origin ----------------------------------------------

    #[cfg(target_os = "linux")]
    #[test]
    fn ip_addr_marks_the_dhcp_assigned_interface_only() {
        let text = "\
1: lo: <LOOPBACK,UP,LOWER_UP> mtu 65536 qdisc noqueue state UNKNOWN group default qlen 1000
    link/loopback 00:00:00:00:00:00 brd 00:00:00:00:00:00
    inet 127.0.0.1/8 scope host lo
3: enp0s3: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 qdisc pfifo_fast state UP group default
    link/ether 08:00:27:aa:bb:cc brd ff:ff:ff:ff:ff:ff
    inet 192.168.1.23/24 brd 192.168.1.255 scope global dynamic enp0s3
4: eth0: <BROADCAST,MULTICAST> mtu 1500 qdisc mq state DOWN group default
    inet 10.0.0.5/24 brd 10.0.0.255 scope global eth0
";
        let mut out = std::collections::BTreeSet::new();
        collect_ip_dynamic(text, &mut out);
        assert!(out.contains("enp0s3"), "{out:?}");
        assert!(!out.contains("eth0"), "a static address is not dynamic");
        assert!(!out.contains("lo"), "loopback is never DHCP");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ip_addr_ignores_link_local_and_other_scopes() {
        // A `dynamic` link-local address says nothing about how the routable
        // address on the same interface was configured.
        let text = "\
3: enp0s3: <BROADCAST,UP> mtu 1500 state UP
    inet 169.254.10.1/16 scope link dynamic enp0s3
    inet 192.168.1.23/24 brd 192.168.1.255 scope global enp0s3
";
        let mut out = std::collections::BTreeSet::new();
        collect_ip_dynamic(text, &mut out);
        assert!(out.is_empty(), "{out:?}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ip_addr_handles_the_ifname_suffix_on_virtual_links() {
        // `eth0@if42:` is the header a veth peer prints.
        let text = "\
7: eth0@if42: <BROADCAST,MULTICAST,UP,LOWER_UP> mtu 1500 state UP
    inet 192.168.7.8/24 brd 192.168.7.255 scope global dynamic eth0
";
        let mut out = std::collections::BTreeSet::new();
        collect_ip_dynamic(text, &mut out);
        assert!(
            out.contains("eth0"),
            "the @if suffix must be stripped: {out:?}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn nmcli_auto_means_dhcp_and_anything_else_does_not() {
        let text =
            "auto:enp0s3\nmanual:eth0\ndisabled:wlan0\nlink-local:virbr0\nauto:enp0s4:enp0s5\n";
        let mut out = std::collections::BTreeSet::new();
        collect_nmcli_dynamic(text, &mut out);
        assert!(out.contains("enp0s3"), "{out:?}");
        // A bridge binding can report several devices on one connection.
        assert!(out.contains("enp0s4") && out.contains("enp0s5"), "{out:?}");
        assert!(!out.contains("eth0"), "manual is static");
        assert!(!out.contains("wlan0"), "disabled is not dhcp");
        assert!(!out.contains("virbr0"), "link-local is not dhcp");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_tool_that_said_nothing_leaves_the_interface_unlabelled() {
        // The whole point of only recording positives: a machine with neither
        // tool must not have its static-looking addresses called "not DHCP".
        let mut out = std::collections::BTreeSet::new();
        collect_ip_dynamic("", &mut out);
        collect_nmcli_dynamic("unparsable output\n", &mut out);
        assert!(out.is_empty(), "{out:?}");
    }
}
