//! Windows network probes: the Winsock resolver, `DnsFlushResolverCache` from
//! `dnsapi`, and the IP helper ICMP API for ping and traceroute.
//!
//! The ICMP API works from an unprivileged token, which is why the probes are
//! native instead of spawning `ping.exe` / `tracert.exe`: both write localized
//! banners that no parser survives (the `route print` lesson).

use super::buffer::AlignedBuffer;
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, UdpSocket};
use std::sync::Once;
use windows_sys::Win32::Foundation::{GetLastError, HANDLE};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    Icmp6CreateFile, Icmp6SendEcho2, IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho,
    ICMPV6_ECHO_REPLY_LH, ICMP_ECHO_REPLY, IP_OPTION_INFORMATION, IP_REQ_TIMED_OUT,
};
use windows_sys::Win32::Networking::WinSock::{
    freeaddrinfo, getaddrinfo, getnameinfo, ADDRINFOA, AF_INET, AF_INET6, SOCKADDR, SOCKADDR_IN,
    SOCKADDR_IN6,
};
use x_core::error::{Error, PermissionRequirement, Result};
use x_core::network::{PingReply, PingRequest, PingSummary, TraceHop};

/// Initialize Winsock once per process; every `ws2_32` call, including the
/// IP helper ICMP APIs, requires it.
fn winsock_init() {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        // SAFETY: WSAStartup only needs a version request and a live buffer.
        unsafe {
            let mut data: windows_sys::Win32::Networking::WinSock::WSADATA = std::mem::zeroed();
            // MAKEWORD(2, 2) is not projected by windows-sys; the value is 0x0202.
            windows_sys::Win32::Networking::WinSock::WSAStartup(0x0202, &mut data);
        }
    });
}

/// Resolve a host name through the platform resolver (`getaddrinfo`).
pub(crate) fn resolve(host: &str) -> Result<Vec<IpAddr>> {
    winsock_init();
    if host.is_empty() {
        return Err(Error::invalid_input("host must not be empty"));
    }
    // getaddrinfo wants the ASCII form of the host name; IDN is out of scope.
    let c_host = std::ffi::CString::new(host)
        .map_err(|_| Error::invalid_input("host contains a NUL byte"))?;
    let hints = ADDRINFOA {
        ai_family: 0, // AF_UNSPEC: report both families
        ai_socktype: 0,
        ai_protocol: 0,
        ai_addrlen: 0,
        ai_canonname: std::ptr::null_mut(),
        ai_addr: std::ptr::null_mut(),
        ai_next: std::ptr::null_mut(),
        ai_flags: 0,
    };
    let mut result: *mut ADDRINFOA = std::ptr::null_mut();
    // SAFETY: the hints struct and the out pointer are valid for the call;
    // the returned list is freed below.
    let rc = unsafe {
        getaddrinfo(
            c_host.as_ptr().cast(),
            std::ptr::null(),
            &hints,
            &mut result,
        )
    };
    if rc != 0 {
        // 11001 WSAHOST_NOT_FOUND / 11004 WSANO_DATA: the name exists but no
        // answer, which scripts must be able to tell from a resolver outage.
        if matches!(rc, 11001 | 11004) {
            return Err(Error::not_found(format!("no address found for `{host}`")));
        }
        return Err(Error::system(format!(
            "getaddrinfo for `{host}` failed (winsock error {rc})"
        )));
    }

    let mut addresses = Vec::new();
    let mut node = result;
    while !node.is_null() {
        // SAFETY: the list is terminated by the API and owned by us now.
        let entry = unsafe { &*node };
        if let Some(address) = decode_sockaddr(entry.ai_addr, entry.ai_addrlen as usize) {
            addresses.push(address);
        }
        node = entry.ai_next;
    }
    // SAFETY: the list came from getaddrinfo and is freed exactly once.
    unsafe { freeaddrinfo(result) };
    addresses.sort_by_key(|a| a.to_string());
    addresses.dedup();
    Ok(addresses)
}

/// Reverse resolution of one address (`getnameinfo` without `NI_NAMEREQD`, so
/// a miss answers with the numeric form instead of failing).
pub(crate) fn reverse_dns(address: IpAddr) -> Result<String> {
    winsock_init();
    // The sockaddr must outlive the call, so it is built as a concrete value.
    let storage: SockStorage = address.into();
    let mut host = [0u8; 256];
    // SAFETY: the sockaddr is fully initialized for its family, the buffers
    // are ours, and the length matches the family.
    let rc = unsafe {
        getnameinfo(
            storage.as_ptr(),
            storage.length(),
            host.as_mut_ptr(),
            host.len() as u32,
            std::ptr::null_mut(),
            0,
            0,
        )
    };
    if rc != 0 {
        return Err(Error::system(format!(
            "getnameinfo for {address} failed (winsock error {rc})"
        )));
    }
    let end = host.iter().position(|b| *b == 0).unwrap_or(host.len());
    let name = String::from_utf8_lossy(&host[..end]).into_owned();
    if name.is_empty() || name == address.to_string() {
        return Err(Error::not_found(format!("no name found for {address}")));
    }
    Ok(name)
}

// Ask the DNS client service to drop its cache.
//
// windows-sys does not project `dnsapi`, so the export is declared here; it
// is a plain `DWORD (void)` call on the DLL Windows already has loaded.
#[link(name = "dnsapi")]
extern "system" {
    fn DnsFlushResolverCache() -> u32;
}

pub(crate) fn flush_dns_cache() -> Result<()> {
    // SAFETY: the function takes no arguments; the return value is checked.
    let rc = unsafe { DnsFlushResolverCache() };
    if rc != 0 {
        // A standard token fails the call; observed error codes are 5
        // (`ERROR_ACCESS_DENIED`) and 1 (`ERROR_INVALID_FUNCTION`), which the
        // DNS client service uses when the RPC caller is not elevated.
        if rc == 5 || rc == 1 {
            return Err(Error::permission_denied(
                PermissionRequirement::Administrator,
                "flushing the DNS cache requires an elevated token",
            ));
        }
        return Err(Error::system(format!(
            "DnsFlushResolverCache failed ({rc})"
        )));
    }
    Ok(())
}

/// An owned IP helper ICMP handle.
struct IcmpHandle(HANDLE);

impl IcmpHandle {
    fn open_v4() -> Result<Self> {
        // SAFETY: IcmpCreateFile returns INVALID_HANDLE_VALUE on failure.
        let handle = unsafe { IcmpCreateFile() };
        if handle.is_null() || handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            return Err(system_error("IcmpCreateFile failed"));
        }
        Ok(Self(handle))
    }

    fn open_v6() -> Result<Self> {
        // SAFETY: as above, for the IPv6 factory.
        let handle = unsafe { Icmp6CreateFile() };
        if handle.is_null() || handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            return Err(system_error("Icmp6CreateFile failed"));
        }
        Ok(Self(handle))
    }
}

impl Drop for IcmpHandle {
    fn drop(&mut self) {
        // SAFETY: the handle came from IcmpCreateFile/Icmp6CreateFile.
        unsafe { IcmpCloseHandle(self.0) };
    }
}

/// `ICMP_ECHO_REPLY.Status` for a successful echo reply.
const IP_SUCCESS: u32 = 0;

/// Send `count` IPv4 echo requests through the IP helper API.
pub(crate) fn ping(request: &PingRequest) -> Result<PingSummary> {
    match request.address {
        IpAddr::V4(v4) => ping_v4(request, v4),
        IpAddr::V6(v6) => ping_v6(request, v6),
    }
}

fn ping_v4(request: &PingRequest, address: Ipv4Addr) -> Result<PingSummary> {
    winsock_init();
    let handle = IcmpHandle::open_v4()?;
    let payload = b"x-ping-payload";
    // The reply buffer receives one ICMP_ECHO_REPLY plus the echoed data.
    let reply_size = std::mem::size_of::<ICMP_ECHO_REPLY>() + payload.len() + 8;
    let mut summary = PingSummary {
        address: address.into(),
        transmitted: 0,
        replies: Vec::new(),
    };

    for sequence in 1..=request.count {
        summary.transmitted += 1;
        let mut reply = AlignedBuffer::zeroed(reply_size);
        // SAFETY: the buffer is larger than the reply record and the API only
        // writes the echoed payload after it.
        let records = unsafe {
            IcmpSendEcho(
                handle.0,
                u32::from(address).to_be(),
                payload.as_ptr().cast(),
                payload.len() as u16,
                std::ptr::null(),
                reply.as_mut_ptr(),
                reply_size as u32,
                request.timeout_ms,
            )
        };
        if records == 0 {
            continue;
        }
        // SAFETY: the API reported at least one record of this type.
        let record = unsafe { reply.read::<ICMP_ECHO_REPLY>() };
        if record.Status != IP_SUCCESS {
            continue;
        }
        summary.replies.push(PingReply {
            sequence,
            address: Ipv4Addr::from(record.Address.to_ne_bytes()).into(),
            rtt_ms: record.RoundTripTime as f64,
            ttl: (record.Options.Ttl > 0).then_some(record.Options.Ttl),
        });
    }
    Ok(summary)
}

fn ping_v6(request: &PingRequest, address: Ipv6Addr) -> Result<PingSummary> {
    winsock_init();
    let handle = IcmpHandle::open_v6()?;
    let payload = b"x-ping-payload";
    let reply_size = std::mem::size_of::<ICMPV6_ECHO_REPLY_LH>() + payload.len() + 8;

    // `Icmp6SendEcho2` insists on a source interface address. Connecting a UDP
    // socket (which never sends) and reading `local_addr` is the portable way
    // to ask the stack which source it would pick, exactly like `ping -6`.
    let probe = UdpSocket::bind("[::]:0")
        .map_err(|err| Error::system(format!("could not open a probe socket: {err}")))?;
    probe
        .connect(std::net::SocketAddrV6::new(address, 0, 0, 0))
        .map_err(|err| Error::system(format!("no IPv6 route to {address}: {err}")))?;
    let IpAddr::V6(source_address) = probe
        .local_addr()
        .map_err(|err| Error::system(format!("local_addr: {err}")))?
        .ip()
    else {
        return Err(Error::system("the IPv6 probe picked an IPv4 source"));
    };

    let source = SockStorage::from(IpAddr::V6(source_address));
    let destination = SockStorage::from(IpAddr::V6(address));
    let mut summary = PingSummary {
        address: address.into(),
        transmitted: 0,
        replies: Vec::new(),
    };

    for sequence in 1..=request.count {
        summary.transmitted += 1;
        let mut reply = AlignedBuffer::zeroed(reply_size);
        // SAFETY: all buffers and sockaddrs outlive the call; no event or
        // completion routine is requested.
        let records = unsafe {
            Icmp6SendEcho2(
                handle.0,
                std::ptr::null_mut(),
                None,
                std::ptr::null(),
                source.as_ptr().cast::<SOCKADDR_IN6>(),
                destination.as_ptr().cast::<SOCKADDR_IN6>(),
                payload.as_ptr().cast(),
                payload.len() as u16,
                std::ptr::null(),
                reply.as_mut_ptr(),
                reply_size as u32,
                request.timeout_ms,
            )
        };
        if records == 0 {
            continue;
        }
        // SAFETY: the API reported at least one record of this type.
        let record = unsafe { reply.read::<ICMPV6_ECHO_REPLY_LH>() };
        if record.Status != IP_SUCCESS {
            continue;
        }
        summary.replies.push(PingReply {
            sequence,
            // The record echoes the responder in a fixed struct; for v6 the
            // destination is the useful label for anycast replies too.
            address: address.into(),
            rtt_ms: record.RoundTripTime as f64,
            ttl: None,
        });
    }
    Ok(summary)
}

/// Trace an IPv4 path with the TTL trick on `IcmpSendEcho`.
///
/// With `IP_OPTION_INFORMATION.Ttl` set to `n`, the first router that would
/// drop the packet answers `time exceeded`, and the ICMP API surfaces the
/// responder in the very same echo-reply record: status 0 is the destination,
/// status `IP_REQ_TIMED_OUT` distinguishes silence, and every other ICMP error
/// code (11041 `IP_TIME_EXCEEDED` among them) carries the responding router.
pub(crate) fn trace(address: IpAddr, max_hops: u32, timeout_ms: u32) -> Result<Vec<TraceHop>> {
    match address {
        IpAddr::V4(v4) => trace_v4(v4, max_hops, timeout_ms),
        IpAddr::V6(_) => Err(Error::unsupported(
            "Windows route tracing is implemented for IPv4 only",
        )),
    }
}

fn trace_v4(address: Ipv4Addr, max_hops: u32, timeout_ms: u32) -> Result<Vec<TraceHop>> {
    winsock_init();
    let handle = IcmpHandle::open_v4()?;
    let payload = b"x-trace-payload";
    let reply_size = std::mem::size_of::<ICMP_ECHO_REPLY>() + payload.len() + 8;
    let mut hops = Vec::new();

    for ttl in 1..=max_hops {
        let options = IP_OPTION_INFORMATION {
            Ttl: ttl as u8,
            Tos: 0,
            Flags: 0,
            OptionsSize: 0,
            OptionsData: std::ptr::null_mut(),
        };
        let mut reply = AlignedBuffer::zeroed(reply_size);
        // SAFETY: buffers outlive the call; options is a plain value struct.
        let records = unsafe {
            IcmpSendEcho(
                handle.0,
                u32::from(address).to_be(),
                payload.as_ptr().cast(),
                payload.len() as u16,
                &options,
                reply.as_mut_ptr(),
                reply_size as u32,
                timeout_ms,
            )
        };
        let hop = if records == 0 {
            TraceHop {
                index: ttl,
                address: None,
                rtt_ms: None,
                reached: false,
            }
        } else {
            // SAFETY: at least one record of this type was written.
            let record = unsafe { reply.read::<ICMP_ECHO_REPLY>() };
            let responder: IpAddr = Ipv4Addr::from(record.Address.to_ne_bytes()).into();
            match record.Status {
                IP_SUCCESS => TraceHop {
                    index: ttl,
                    address: Some(responder),
                    rtt_ms: Some(record.RoundTripTime as f64),
                    reached: true,
                },
                IP_REQ_TIMED_OUT => TraceHop {
                    index: ttl,
                    address: None,
                    rtt_ms: None,
                    reached: false,
                },
                // Any other ICMP error carries the responding router in
                // `Address`.
                _ => TraceHop {
                    index: ttl,
                    address: Some(responder),
                    rtt_ms: Some(record.RoundTripTime as f64),
                    reached: false,
                },
            }
        };
        let reached = hop.reached;
        hops.push(hop);
        if reached {
            break;
        }
    }
    Ok(hops)
}

/// A sockaddr that can be built from an `IpAddr` and read as a raw pointer.
enum SockStorage {
    V4(SOCKADDR_IN),
    V6(SOCKADDR_IN6),
}

impl From<IpAddr> for SockStorage {
    fn from(address: IpAddr) -> Self {
        match address {
            IpAddr::V4(v4) => {
                // A zeroed sockaddr is valid to fill field by field; the
                // `S_addr` arm is the one the family tag selects.
                let mut inner: SOCKADDR_IN = unsafe { std::mem::zeroed() };
                inner.sin_family = AF_INET;
                inner.sin_addr.S_un.S_addr = u32::from(v4).to_be();
                SockStorage::V4(inner)
            }
            IpAddr::V6(v6) => {
                // As above, for the IPv6 arm.
                let mut inner: SOCKADDR_IN6 = unsafe { std::mem::zeroed() };
                inner.sin6_family = AF_INET6;
                inner.sin6_addr.u.Byte = v6.octets();
                SockStorage::V6(inner)
            }
        }
    }
}

impl SockStorage {
    fn as_ptr(&self) -> *const SOCKADDR {
        match self {
            SockStorage::V4(inner) => inner as *const SOCKADDR_IN as *const SOCKADDR,
            SockStorage::V6(inner) => inner as *const SOCKADDR_IN6 as *const SOCKADDR,
        }
    }

    fn length(&self) -> i32 {
        match self {
            SockStorage::V4(_) => std::mem::size_of::<SOCKADDR_IN>() as i32,
            SockStorage::V6(_) => std::mem::size_of::<SOCKADDR_IN6>() as i32,
        }
    }
}

/// Decode a raw `sockaddr` of the given length into an address.
pub(crate) fn decode_sockaddr(ptr: *mut SOCKADDR, length: usize) -> Option<IpAddr> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: `length` comes from the API and tells which arm is complete.
    unsafe {
        match (*ptr).sa_family {
            AF_INET if length >= std::mem::size_of::<SOCKADDR_IN>() => {
                let inner = &*(ptr as *const SOCKADDR_IN);
                Some(Ipv4Addr::from(inner.sin_addr.S_un.S_addr.to_be()).into())
            }
            AF_INET6 if length >= std::mem::size_of::<SOCKADDR_IN6>() => {
                let inner = &*(ptr as *const SOCKADDR_IN6);
                Some(Ipv6Addr::from(inner.sin6_addr.u.Byte).into())
            }
            _ => None,
        }
    }
}

/// Turn the calling thread's last error into a system error.
fn system_error(context: &str) -> Error {
    // SAFETY: GetLastError is always callable.
    let code = unsafe { GetLastError() };
    let kind = match code {
        5 => (
            x_core::error::ErrorKind::PermissionDenied,
            PermissionRequirement::Administrator,
        ),
        _ => (
            x_core::error::ErrorKind::System,
            PermissionRequirement::None,
        ),
    };
    Error::new(
        kind.0,
        format!("{context} ({})", io::Error::from_raw_os_error(code as i32)),
    )
    .with_permission(kind.1)
}
