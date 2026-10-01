//! Shared `getifaddrs` traversal for the BSD-derived platforms.
//!
//! macOS and Linux expose the same interface enumeration API, so the walk, the
//! address decoding and the [`InterfaceInfo`]/[`AddressInfo`] mapping are written
//! once here. Only three things differ between the two and they are isolated
//! below: the `sockaddr` prefix, the link level address layout, and where the
//! interface MTU comes from.

use std::collections::BTreeMap;
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
            .or_insert_with(|| InterfaceInfo {
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
                received_bytes: None,
                transmitted_bytes: None,
            });
    }

    Ok(rows.into_values().collect())
}

/// Enumerate configured IP addresses with their prefix lengths.
pub fn addresses() -> Result<Vec<AddressInfo>> {
    let list = IfAddrs::new()?;
    let _guard = IfAddrsGuard(list.head);

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
        rows.push(AddressInfo {
            interface: entry.name,
            address,
            prefix_len,
            dhcp: None,
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
}
