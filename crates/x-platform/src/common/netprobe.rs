//! Network probes shared by the BSD-derived platforms.
//!
//! macOS and Linux both expose the POSIX resolver (`getaddrinfo`/`getnameinfo`)
//! and unprivileged DGRAM ICMP sockets, so the ping and traceroute machinery is
//! written once here. What stays platform specific lives in the adapters:
//! flushing the resolver cache has no common entry point.
//!
//! The sockets are `SOCK_DGRAM + IPPROTO_ICMP(_V6)` rather than `SOCK_RAW`:
//! the kernel rewrites the echo identifier and computes the checksum, so no
//! process needs root just to ask a host whether it is alive. Where the kernel
//! refuses (SELinux policies, containers, older macOS), the error surfaces as a
//! permission error instead of silently falling back to spawning `ping`.

use libc::{c_int, c_void, sockaddr, sockaddr_in, sockaddr_in6, socklen_t};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Instant;
use x_core::error::{Error, PermissionRequirement, Result};
use x_core::network::{PingReply, PingRequest, PingSummary, TraceHop};

/// Resolve a host name through the platform resolver.
pub fn resolve(host: &str) -> Result<Vec<IpAddr>> {
    if host.is_empty() {
        return Err(Error::invalid_input("host must not be empty"));
    }
    let c_host = std::ffi::CString::new(host)
        .map_err(|_| Error::invalid_input("host contains a NUL byte"))?;
    let mut hints: libc::addrinfo = unsafe { std::mem::zeroed() };
    hints.ai_family = libc::AF_UNSPEC;
    let mut result: *mut libc::addrinfo = std::ptr::null_mut();
    // SAFETY: `hints` and the out pointer are valid for the call and the
    // returned list is freed below.
    let rc = unsafe { libc::getaddrinfo(c_host.as_ptr(), std::ptr::null(), &hints, &mut result) };
    if rc != 0 {
        // A name with no records is a not-found answer, not a resolver
        // outage: `EAI_NONAME`/`EAI_NODATA` must not look like a crash.
        if rc == libc::EAI_NONAME || rc == libc::EAI_NODATA {
            return Err(Error::not_found(format!("no address found for `{host}`")));
        }
        // SAFETY: the error number refers to a static table.
        let text = unsafe { libc::gai_strerror(rc) };
        let reason = if text.is_null() {
            format!("code {rc}")
        } else {
            // SAFETY: `gai_strerror` returns a NUL terminated C string.
            unsafe { std::ffi::CStr::from_ptr(text) }
                .to_string_lossy()
                .into_owned()
        };
        return Err(Error::system(format!("getaddrinfo for `{host}`: {reason}")));
    }

    let mut addresses = Vec::new();
    let mut node = result;
    while !node.is_null() {
        // SAFETY: the list is terminated by the API and owned by us now.
        let entry = unsafe { &*node };
        if let Some(address) = decode_sockaddr(
            entry.ai_addr as *const sockaddr,
            entry.ai_addrlen as socklen_t,
        ) {
            addresses.push(address);
        }
        node = entry.ai_next;
    }
    // SAFETY: the list came from getaddrinfo and is freed exactly once.
    unsafe { libc::freeaddrinfo(result) };
    addresses.sort_by_key(|a| a.to_string());
    addresses.dedup();
    Ok(addresses)
}

/// Reverse resolution of one address.
///
/// `NI_NAMEREQD` makes a resolver miss an explicit failure, which is what
/// `x net reverse` reports; without it the function answers with the numeric
/// form and the caller cannot tell a PTR record from the input.
pub fn reverse_dns(address: IpAddr) -> Result<String> {
    let storage = SockStorage::from(address);
    let mut host = [0u8; libc::NI_MAXHOST as usize];
    // SAFETY: the sockaddr is fully initialized for its family and the buffer
    // length is declared.
    let rc = unsafe {
        libc::getnameinfo(
            storage.as_ptr(),
            storage.length(),
            host.as_mut_ptr().cast(),
            libc::NI_MAXHOST as socklen_t,
            std::ptr::null_mut(),
            0,
            libc::NI_NAMEREQD,
        )
    };
    if rc != 0 {
        return Err(Error::not_found(format!("no name found for {address}")));
    }
    // SAFETY: getnameinfo wrote a NUL terminated name into our buffer.
    let name = unsafe { std::ffi::CStr::from_ptr(host.as_ptr().cast()) }
        .to_string_lossy()
        .into_owned();
    if name.is_empty() {
        return Err(Error::not_found(format!("no name found for {address}")));
    }
    Ok(name)
}

/// Send `count` echo requests and collect the replies.
pub fn ping(request: &PingRequest) -> Result<PingSummary> {
    let socket = Probe::open(request.address, request.timeout_ms)?;
    let payload = b"x-ping-payload";
    let identifier = std::process::id() as u16;
    let mut summary = PingSummary {
        address: request.address,
        transmitted: 0,
        replies: Vec::new(),
    };

    for sequence in 1..=request.count {
        summary.transmitted += 1;
        let packet = echo_message(
            request.address.is_ipv6(),
            identifier,
            sequence as u16,
            payload,
        );
        if let Some(reply) = socket.send_and_receive(&packet) {
            if reply.kind == ProbeKind::Echo {
                summary.replies.push(PingReply {
                    sequence,
                    address: reply.address,
                    rtt_ms: reply.rtt_ms,
                    // The DGRAM ICMP socket reports the peer but not the
                    // inbound hop limit; a per-hop TTL adds no value to a
                    // ping summary, so the field stays empty rather than
                    // invented.
                    ttl: None,
                });
            }
        }
    }
    Ok(summary)
}

/// Trace the path to `address` with the TTL trick on echo requests.
pub fn trace(address: IpAddr, max_hops: u32, timeout_ms: u32) -> Result<Vec<TraceHop>> {
    let socket = Probe::open(address, timeout_ms)?;
    let payload = b"x-trace-payload";
    let identifier = std::process::id() as u16;
    let mut hops = Vec::new();

    for hop in 1..=max_hops {
        socket.set_hop_limit(hop)?;
        let packet = echo_message(address.is_ipv6(), identifier, hop as u16, payload);
        let reply = socket.send_and_receive(&packet);
        let hop = match reply {
            Some(reply) if reply.kind == ProbeKind::Echo => TraceHop {
                index: hop,
                address: Some(reply.address),
                rtt_ms: Some(reply.rtt_ms),
                reached: true,
            },
            Some(reply) => TraceHop {
                index: hop,
                address: Some(reply.address),
                rtt_ms: Some(reply.rtt_ms),
                reached: false,
            },
            None => TraceHop {
                index: hop,
                address: None,
                rtt_ms: None,
                reached: false,
            },
        };
        let reached = hop.reached;
        hops.push(hop);
        if reached {
            break;
        }
    }
    Ok(hops)
}

/// What an ICMP receive told us.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProbeKind {
    /// Echo reply: the destination answered.
    Echo,
    /// ICMP error (time exceeded, destination unreachable, ...): an
    /// intermediate router answered.
    Error,
}

/// One answered probe.
struct ProbeReply {
    address: IpAddr,
    rtt_ms: f64,
    kind: ProbeKind,
}

/// A DGRAM ICMP socket with a fixed destination and receive timeout.
struct Probe {
    fd: c_int,
    destination: IpAddr,
    v6: bool,
}

impl Probe {
    fn open(destination: IpAddr, timeout_ms: u32) -> Result<Self> {
        let v6 = destination.is_ipv6();
        let (family, protocol) = if v6 {
            (libc::AF_INET6, libc::IPPROTO_ICMPV6)
        } else {
            (libc::AF_INET, libc::IPPROTO_ICMP)
        };
        // SAFETY: plain socket creation; the descriptor is closed in `Drop`.
        let fd = unsafe { libc::socket(family, libc::SOCK_DGRAM, protocol) };
        if fd < 0 {
            let err = std::io::Error::last_os_error();
            if matches!(err.raw_os_error(), Some(libc::EACCES | libc::EPERM)) {
                return Err(Error::permission_denied(
                    PermissionRequirement::Root,
                    format!("unprivileged ICMP sockets are not allowed: {err}"),
                ));
            }
            return Err(Error::system(format!(
                "could not open an ICMP socket: {err}"
            )));
        }
        let socket = Self {
            fd,
            destination,
            v6,
        };
        socket.set_receive_timeout(timeout_ms);
        Ok(socket)
    }

    fn set_receive_timeout(&self, timeout_ms: u32) {
        let timeout = libc::timeval {
            tv_sec: (timeout_ms / 1000) as libc::time_t,
            tv_usec: ((timeout_ms % 1000) * 1000) as _,
        };
        // SAFETY: `timeout` is a valid timeval for the option.
        unsafe {
            libc::setsockopt(
                self.fd,
                libc::SOL_SOCKET,
                libc::SO_RCVTIMEO,
                &timeout as *const libc::timeval as *const c_void,
                std::mem::size_of::<libc::timeval>() as socklen_t,
            )
        };
    }

    /// Set the outbound hop limit for the next sends (the traceroute dial).
    fn set_hop_limit(&self, hop: u32) -> Result<()> {
        let (level, option) = if self.v6 {
            (libc::IPPROTO_IPV6, libc::IPV6_UNICAST_HOPS)
        } else {
            (libc::IPPROTO_IP, libc::IP_TTL)
        };
        let value = hop as c_int;
        // SAFETY: an int option of the declared size.
        let rc = unsafe {
            libc::setsockopt(
                self.fd,
                level,
                option,
                &value as *const c_int as *const c_void,
                std::mem::size_of::<c_int>() as socklen_t,
            )
        };
        if rc != 0 {
            return Err(Error::system(format!(
                "setting the hop limit to {hop} failed: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(())
    }

    /// Send one packet and wait for the matching answer; `None` on timeout.
    fn send_and_receive(&self, packet: &[u8]) -> Option<ProbeReply> {
        let storage = SockStorage::from(self.destination);
        // SAFETY: the destination sockaddr is initialized for its family.
        let sent = unsafe {
            libc::sendto(
                self.fd,
                packet.as_ptr() as *const c_void,
                packet.len(),
                0,
                storage.as_ptr(),
                storage.length(),
            )
        };
        if sent < 0 {
            return None;
        }
        let started = Instant::now();

        let mut buffer = [0u8; 2048];
        let mut from: sockaddr = unsafe { std::mem::zeroed() };
        let mut from_len: socklen_t = std::mem::size_of::<sockaddr>() as socklen_t;
        // SAFETY: buffers are ours and sized; the call only writes within them.
        let read = unsafe {
            libc::recvfrom(
                self.fd,
                buffer.as_mut_ptr() as *mut c_void,
                buffer.len(),
                0,
                &mut from,
                &mut from_len,
            )
        };
        if read < 0 {
            return None;
        }
        // The socket is connected to nobody, so replies arrive addressed by the
        // responder; that is exactly the label the frontend prints.
        let address = decode_sockaddr(&from, from_len)?;
        let kind = classify_icmp(&buffer[..read as usize])?;
        Some(ProbeReply {
            address,
            rtt_ms: started.elapsed().as_secs_f64() * 1000.0,
            kind,
        })
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        // SAFETY: the descriptor came from `socket` above.
        unsafe { libc::close(self.fd) };
    }
}

/// Build an ICMP echo request; the DGRAM kernel path rewrites the checksum
/// and identifier, but a well-formed packet keeps the raw path honest too.
fn echo_message(v6: bool, identifier: u16, sequence: u16, payload: &[u8]) -> Vec<u8> {
    let r#type = if v6 { 128 } else { 8 };
    let mut message = Vec::with_capacity(8 + payload.len());
    message.push(r#type);
    message.push(0);
    message.extend_from_slice(&0u16.to_be_bytes());
    message.extend_from_slice(&identifier.to_be_bytes());
    message.extend_from_slice(&sequence.to_be_bytes());
    message.extend_from_slice(payload);
    let checksum = icmp_checksum(&message);
    message[2..4].copy_from_slice(&checksum.to_be_bytes());
    message
}

/// Classify the first ICMP header of a received datagram.
///
/// A DGRAM ICMP socket delivers the ICMP message itself (the kernel strips the
/// IP header), so byte 0 is the type: `0`/`129` is an echo reply and everything
/// else that reports trouble (time exceeded `11`/`2`, destination unreachable
/// `3`/`1`) means an intermediate router answered.
fn classify_icmp(message: &[u8]) -> Option<ProbeKind> {
    let kind_byte = *message.first()?;
    match kind_byte {
        0 | 129 => Some(ProbeKind::Echo),
        2 | 3 | 11 => Some(ProbeKind::Error),
        _ => None,
    }
}

/// One's complement checksum over an ICMP message.
fn icmp_checksum(message: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut chunks = message.chunks_exact(2);
    for chunk in &mut chunks {
        sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
    }
    if let Some(&last) = chunks.remainder().first() {
        sum += (last as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// A sockaddr that can be built from an `IpAddr` and passed as a raw pointer.
enum SockStorage {
    V4(sockaddr_in),
    V6(sockaddr_in6),
}

impl From<IpAddr> for SockStorage {
    fn from(address: IpAddr) -> Self {
        match address {
            IpAddr::V4(v4) => {
                let mut inner: sockaddr_in = unsafe { std::mem::zeroed() };
                // The family field is `u8` on macOS and `u16` on Linux.
                inner.sin_family = libc::AF_INET as _;
                inner.sin_addr.s_addr = u32::from(v4).to_be();
                SockStorage::V4(inner)
            }
            IpAddr::V6(v6) => {
                let mut inner: sockaddr_in6 = unsafe { std::mem::zeroed() };
                inner.sin6_family = libc::AF_INET6 as _;
                inner.sin6_addr.s6_addr = v6.octets();
                SockStorage::V6(inner)
            }
        }
    }
}

impl SockStorage {
    fn as_ptr(&self) -> *const sockaddr {
        match self {
            SockStorage::V4(inner) => inner as *const sockaddr_in as *const sockaddr,
            SockStorage::V6(inner) => inner as *const sockaddr_in6 as *const sockaddr,
        }
    }

    fn length(&self) -> socklen_t {
        match self {
            SockStorage::V4(_) => std::mem::size_of::<sockaddr_in>() as socklen_t,
            SockStorage::V6(_) => std::mem::size_of::<sockaddr_in6>() as socklen_t,
        }
    }
}

/// Decode a kernel `sockaddr` of the given family into an address.
fn decode_sockaddr(ptr: *const sockaddr, length: socklen_t) -> Option<IpAddr> {
    if ptr.is_null() {
        return None;
    }
    let length = length as usize;
    // SAFETY: `length` comes from the kernel and tells which arm is complete;
    // the family tag selects the concrete sockaddr type.
    unsafe {
        match (*ptr).sa_family as u32 {
            f if f == libc::AF_INET as u32 && length >= std::mem::size_of::<sockaddr_in>() => {
                let inner = &*(ptr as *const sockaddr_in);
                Some(Ipv4Addr::from(u32::from_be(inner.sin_addr.s_addr)).into())
            }
            f if f == libc::AF_INET6 as u32 && length >= std::mem::size_of::<sockaddr_in6>() => {
                let inner = &*(ptr as *const sockaddr_in6);
                Some(Ipv6Addr::from(inner.sin6_addr.s6_addr).into())
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echo_messages_carry_the_family_type_and_payload() {
        // Header layout: type(1) code(1) checksum(2) identifier(2) sequence(2).
        let message = echo_message(false, 7, 3, b"abc");
        assert_eq!(message[0], 8);
        assert_eq!(&message[4..6], &7u16.to_be_bytes(), "identifier");
        assert_eq!(&message[6..8], &3u16.to_be_bytes(), "sequence");
        assert_eq!(&message[8..], b"abc");

        let v6 = echo_message(true, 7, 3, b"abc");
        assert_eq!(v6[0], 128);
    }

    #[test]
    fn the_checksum_verifies_over_the_finished_message() {
        // A receiver computing the checksum over a well-formed message gets a
        // zero remainder; that is the property this function must satisfy.
        let message = echo_message(false, 0x1234, 1, b"payload");
        assert_eq!(icmp_checksum(&message), 0);
        let mut broken = message.clone();
        broken[10] ^= 0xff;
        assert_ne!(icmp_checksum(&broken), 0);
    }

    #[test]
    fn reply_and_error_types_are_classified() {
        assert_eq!(classify_icmp(&[0, 0, 0, 0]), Some(ProbeKind::Echo));
        assert_eq!(classify_icmp(&[129, 0, 0, 0]), Some(ProbeKind::Echo));
        assert_eq!(classify_icmp(&[11, 0, 0, 0]), Some(ProbeKind::Error));
        assert_eq!(classify_icmp(&[2, 0, 0, 0]), Some(ProbeKind::Error));
        assert_eq!(classify_icmp(&[3, 0, 0, 0]), Some(ProbeKind::Error));
        assert_eq!(classify_icmp(&[5, 0, 0, 0]), None);
        assert_eq!(classify_icmp(&[]), None);
    }

    #[test]
    fn sockaddr_round_trips_both_families() {
        let v4 = SockStorage::from(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 5)));
        assert_eq!(
            decode_sockaddr(v4.as_ptr(), v4.length()),
            Some("203.0.113.5".parse::<IpAddr>().unwrap())
        );
        let v6 = SockStorage::from(IpAddr::V6("fe80::1".parse().unwrap()));
        assert_eq!(
            decode_sockaddr(v6.as_ptr(), v6.length()),
            Some("fe80::1".parse::<IpAddr>().unwrap())
        );
    }

    #[test]
    fn resolution_reports_a_real_lookup() {
        // localhost must resolve on every machine, and the resolver is the
        // thing under test rather than the network.
        let addresses = resolve("localhost").expect("resolve localhost");
        assert!(addresses.iter().any(|a| a.is_loopback()), "{addresses:?}");
    }
}
