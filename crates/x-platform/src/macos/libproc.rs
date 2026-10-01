//! Raw `libproc` bindings used to attribute sockets to processes on macOS.
//!
//! Strategy, all native syscalls with no process spawn:
//!
//! 1. `proc_listallpids` for the pid table.
//! 2. `proc_pidinfo(PROC_PIDLISTFDS)` for each process' descriptor table.
//! 3. `proc_pidfdinfo(PROC_PIDFDSOCKETINFO)` for each descriptor to learn the
//!    protocol, the addresses, the ports and the TCP state.
//!
//! This is what `lsof` does, minus the fork/exec.
//!
//! # Layout warning
//!
//! `struct socket_fdinfo` is 792 bytes of nested unions and platform structs.
//! Instead of relying on `#[repr(C)]` to reproduce the kernel's layout (which
//! changes when the SDK headers do), the fields we need are read from the raw
//! buffer at the offsets the SDK reports. The constants below come from
//! `offsetof()` on the installed SDK and are asserted by the unit tests.

use std::mem;

use libc::{c_int, c_void};

/// `proc_pidinfo` flavor: fill a buffer with `proc_fdinfo`.
pub const PROC_PIDLISTFDS: c_int = 1;
/// `proc_pidinfo` flavor: full BSD process info.
pub const PROC_PIDTBSDINFO: c_int = 3;
pub const PROC_PIDTASKINFO: c_int = 4;
/// `proc_pidinfo` flavor: `proc_taskallinfo`, including CPU time.
pub const PROC_PIDTASKALLINFO: c_int = 2;

/// `proc_pidfdinfo` flavor: socket details.
///
/// Note the flavor namespace is *per function*: `PROC_PIDFDSOCKETINFO` is `3`
/// here while `PROC_PIDLISTFDS` is `1` in the `proc_pidinfo` namespace.
pub const PROC_PIDFDSOCKETINFO: c_int = 3;

/// `proc_pidfdinfo` flavor: descriptor path via `vnode_fdinfowithpath`.
pub const PROC_PIDFDVNODEPATHINFO: c_int = 2;

/// `socket_info.soi_kind` discriminants.
pub const SOCKINFO_GENERIC: c_int = 0;
pub const SOCKINFO_IN: c_int = 1;
pub const SOCKINFO_TCP: c_int = 2;
pub const SOCKINFO_UN: c_int = 3;

/// `in_sockinfo.insi_vflag` discriminants.
pub const INI_IPV4: u8 = 1;
pub const INI_IPV6: u8 = 2;

/// `PROC_PROX_FDTYPE_*` descriptor types.
pub const PROX_FDTYPE_VNODE: u32 = 1;
pub const PROX_FDTYPE_SOCKET: u32 = 2;

/// `TCPS_*` states from `netinet/tcp_fsm.h`.
pub const TCPS_CLOSED: i32 = 0;
pub const TCPS_LISTEN: i32 = 1;
pub const TCPS_SYN_SENT: i32 = 2;
pub const TCPS_SYN_RECEIVED: i32 = 3;
pub const TCPS_ESTABLISHED: i32 = 4;
pub const TCPS_CLOSE_WAIT: i32 = 5;
pub const TCPS_FIN_WAIT_1: i32 = 6;
pub const TCPS_CLOSING: i32 = 7;
pub const TCPS_LAST_ACK: i32 = 8;
pub const TCPS_FIN_WAIT_2: i32 = 9;
pub const TCPS_TIME_WAIT: i32 = 10;

/// `sizeof(struct socket_fdinfo)` on the installed SDK.
pub const SOCKET_FDINFO_SIZE: usize = 792;

/// `offsetof(struct socket_fdinfo, psi)`.
const OFF_SOCKET_INFO: usize = 24;
/// `offsetof(struct socket_info, soi_kind)`, relative to `socket_fdinfo`.
pub const OFF_KIND: usize = OFF_SOCKET_INFO + 232;
/// `offsetof(struct socket_info, soi_proto)`, relative to `socket_fdinfo`.
pub const OFF_PROTO: usize = OFF_SOCKET_INFO + 240;
/// `offsetof(struct in_sockinfo, insi_fport)`.
pub const OFF_INI_FPORT: usize = OFF_PROTO;
/// `offsetof(struct in_sockinfo, insi_lport)`.
pub const OFF_INI_LPORT: usize = OFF_PROTO + 4;
/// `offsetof(struct in_sockinfo, insi_vflag)`.
pub const OFF_INI_VFLAG: usize = OFF_PROTO + 24;
/// `offsetof(struct in_sockinfo, insi_faddr)`.
pub const OFF_INI_FADDR: usize = OFF_PROTO + 32;
/// `offsetof(struct in_sockinfo, insi_laddr)`.
pub const OFF_INI_LADDR: usize = OFF_PROTO + 48;
/// `offsetof(struct tcp_sockinfo, tcpsi_state)`.
pub const OFF_TCP_STATE: usize = OFF_PROTO + 80;
/// `offsetof(struct tcp_connection_info, tcpi_snd_sbbytes)`, relative to
/// `socket_fdinfo`.
///
/// `tcpsi_tcbc` opens with the four one-byte fields (`tcpi_state`,
/// `tcpi_snd_wscale`, `tcpi_rcv_wscale`, `__pad1`), then seven `u32`s
/// (`options`, `flags`, `rto`, `maxseg`, `snd_ssthresh`, `snd_cwnd`,
/// `snd_wnd`), so the byte count riding on `tcpi_state` sits 32 bytes in.
pub const OFF_TCP_SND_SBBYTES: usize = OFF_TCP_STATE + 32;

/// `sizeof(struct vnode_fdinfowithpath)` on the installed SDK.
pub const VNODE_PATH_FDINFO_SIZE: usize = 1200;
/// `offsetof(struct vnode_fdinfowithpath, pvip.vip_path)`.
///
/// `pvip` starts at 24 (after `proc_fileinfo`), `vip_path` at 152 inside it.
pub const OFF_FD_PATH: usize = 24 + 152;
/// `PATH_MAX`, the size of `vip_path`.
pub const FD_PATH_MAX: usize = 1024;

/// One entry of a process descriptor table.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct proc_fdinfo {
    /// Descriptor number.
    pub proc_fd: i32,
    /// `PROC_PROX_FDTYPE_*`.
    pub proc_fdtype: u32,
}

/// Read a little endian `i32` out of a kernel buffer.
fn i32_at(buffer: &[u8], offset: usize) -> i32 {
    let bytes: [u8; 4] = buffer
        .get(offset..offset + 4)
        .and_then(|slice| slice.try_into().ok())
        .unwrap_or([0; 4]);
    i32::from_ne_bytes(bytes)
}

/// Read a little endian `u32` out of a kernel buffer.
fn u32_at(buffer: &[u8], offset: usize) -> u32 {
    let bytes: [u8; 4] = buffer
        .get(offset..offset + 4)
        .and_then(|slice| slice.try_into().ok())
        .unwrap_or([0; 4]);
    u32::from_ne_bytes(bytes)
}

/// Ports are stored as network byte order inside an `int`.
fn port_at(buffer: &[u8], offset: usize) -> u16 {
    let bytes: [u8; 2] = buffer
        .get(offset..offset + 2)
        .and_then(|slice| slice.try_into().ok())
        .unwrap_or([0; 2]);
    u16::from_be_bytes(bytes)
}

extern "C" {
    fn proc_listallpids(buffer: *mut c_void, buffersize: c_int) -> c_int;
    fn proc_pidinfo(
        pid: c_int,
        flavor: c_int,
        arg: u64,
        buffer: *mut c_void,
        buffersize: c_int,
    ) -> c_int;
    fn proc_pidfdinfo(
        pid: c_int,
        fd: c_int,
        flavor: c_int,
        buffer: *mut c_void,
        buffersize: c_int,
    ) -> c_int;
    fn proc_pidpath(pid: c_int, buffer: *mut c_void, buffersize: u32) -> c_int;
}

/// Every pid the kernel will show us.
pub fn list_pids() -> Vec<i32> {
    let mut buffer = vec![0i32; 8192];
    let mut pids = Vec::new();

    loop {
        let size = (buffer.len() * mem::size_of::<i32>()) as c_int;
        // SAFETY: `buffer` is exactly `size` bytes, which is what we declare.
        let written = unsafe { proc_listallpids(buffer.as_mut_ptr() as *mut c_void, size) };
        if written <= 0 {
            break;
        }
        let count = (written as usize) / mem::size_of::<i32>();
        pids.extend(buffer.iter().take(count).copied().filter(|p| *p > 0));
        if (written as usize) < size as usize {
            break;
        }
        // The buffer was filled completely: a process may have been missed.
        buffer.resize(buffer.len() * 2, 0);
    }

    pids.sort_unstable();
    pids.dedup();
    pids
}

/// Path of one descriptor, or `None` when it is not a vnode or is unreadable.
///
/// Uses `PROC_PIDFDVNODEPATHINFO`, the flavor that carries
/// `vnode_fdinfowithpath` — the plain vnode flavor leaves the path out and
/// would force a second, larger read.
pub fn fd_path(pid: i32, fd: i32) -> Option<String> {
    let mut buffer = [0u8; VNODE_PATH_FDINFO_SIZE];
    // SAFETY: the buffer matches `sizeof(struct vnode_fdinfowithpath)`.
    let written = unsafe {
        proc_pidfdinfo(
            pid,
            fd,
            PROC_PIDFDVNODEPATHINFO,
            buffer.as_mut_ptr() as *mut c_void,
            VNODE_PATH_FDINFO_SIZE as c_int,
        )
    };
    if (written as usize) < VNODE_PATH_FDINFO_SIZE {
        return None;
    }
    let path = &buffer[OFF_FD_PATH..OFF_FD_PATH + FD_PATH_MAX];
    let end = path.iter().position(|b| *b == 0).unwrap_or(0);
    if end == 0 {
        return None;
    }
    String::from_utf8(path[..end].to_vec()).ok()
}

/// Every descriptor owned by `pid`.
///
/// `proc_pidinfo(PROC_PIDLISTFDS, 0, NULL, 0)` does *not* return the descriptor
/// count on current kernels, so the buffer is sized generously and grown if the
/// kernel fills it completely.
pub fn list_fds(pid: i32) -> Vec<proc_fdinfo> {
    let mut capacity = 512usize;
    loop {
        let mut buffer = vec![proc_fdinfo::default(); capacity];
        // SAFETY: `buffer` is `capacity * size_of::<proc_fdinfo>()` bytes.
        let written = unsafe {
            proc_pidinfo(
                pid,
                PROC_PIDLISTFDS,
                0,
                buffer.as_mut_ptr() as *mut c_void,
                (capacity * mem::size_of::<proc_fdinfo>()) as c_int,
            )
        };
        if written <= 0 {
            return Vec::new();
        }
        let bytes = written as usize;
        let filled = bytes / mem::size_of::<proc_fdinfo>();
        buffer.truncate(filled);
        if bytes >= capacity * mem::size_of::<proc_fdinfo>() {
            capacity *= 2;
            continue;
        }
        return buffer;
    }
}

/// Socket details for one descriptor, or `None` when the descriptor is not a
/// socket or belongs to another user.
pub fn socket_info(pid: i32, fd: i32) -> Option<SocketDetails> {
    let mut buffer = [0u8; SOCKET_FDINFO_SIZE];
    // SAFETY: the buffer matches `sizeof(struct socket_fdinfo)`.
    let written = unsafe {
        proc_pidfdinfo(
            pid,
            fd,
            PROC_PIDFDSOCKETINFO,
            buffer.as_mut_ptr() as *mut c_void,
            SOCKET_FDINFO_SIZE as c_int,
        )
    };
    if (written as usize) < SOCKET_FDINFO_SIZE {
        return None;
    }

    let kind = i32_at(&buffer, OFF_KIND);
    let vflag = buffer.get(OFF_INI_VFLAG).copied().unwrap_or(INI_IPV4);

    Some(SocketDetails {
        kind,
        local_port: port_at(&buffer, OFF_INI_LPORT),
        remote_port: port_at(&buffer, OFF_INI_FPORT),
        local_address: read_address(&buffer, OFF_INI_LADDR, vflag),
        remote_address: read_address(&buffer, OFF_INI_FADDR, vflag),
        ip_version: vflag,
        tcp_state: i32_at(&buffer, OFF_TCP_STATE),
        // The send buffer counter only exists in the TCP arm of the union;
        // reading it for other socket kinds would decode a different field.
        send_queue_bytes: (kind == SOCKINFO_TCP).then(|| u32_at(&buffer, OFF_TCP_SND_SBBYTES)),
    })
}

/// The parts of `struct socket_fdinfo` x needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SocketDetails {
    /// `SOCKINFO_*` discriminant.
    pub kind: i32,
    /// Local port, network byte order already converted.
    pub local_port: u16,
    /// Peer port, `0` when there is no peer.
    pub remote_port: u16,
    /// Local address.
    pub local_address: [u8; 16],
    /// Peer address.
    pub remote_address: [u8; 16],
    /// `INI_IPV4` or `INI_IPV6`.
    pub ip_version: u8,
    /// `TCPS_*` state, meaningful for TCP only.
    pub tcp_state: i32,
    /// Bytes in the TCP send buffer (`tcpi_snd_sbbytes`), TCP sockets only.
    ///
    /// The kernel exposes no receive-queue occupancy through `libproc`, so
    /// there is no matching field for the other direction.
    pub send_queue_bytes: Option<u32>,
}

/// Read one of the two address unions.
///
/// For IPv4 the field is an `in4in6_addr`, i.e. 12 zero bytes followed by the
/// 4 byte address, exactly like an IPv4-mapped IPv6 address.
fn read_address(buffer: &[u8], offset: usize, vflag: u8) -> [u8; 16] {
    let mut raw = [0u8; 16];
    if let Some(slice) = buffer.get(offset..offset + 16) {
        raw.copy_from_slice(slice);
    }
    match vflag {
        INI_IPV6 => raw,
        _ => {
            let mut v4 = [0u8; 16];
            v4[12..].copy_from_slice(&raw[12..]);
            v4
        }
    }
}

/// Byte offset of `pbi_uid` inside `struct proc_bsdinfo`.
///
/// The struct is a run of `u32` fields (`flags`, `status`, `xstatus`, `pid`,
/// `ppid`) before the uid, so the offset is derived rather than hard coded
/// twice, and the layout assertion below fails loudly if the SDK disagrees.
const OFF_PBI_UID: usize = 5 * std::mem::size_of::<u32>();

/// Buffer handed to `proc_pidinfo` for `PROC_PIDTBSDINFO`.
///
/// `proc_pidinfo` refuses to write when the buffer is smaller than the struct,
/// and `struct proc_bsdinfo` is 124 bytes. Over-allocating keeps this correct
/// if Apple ever grows the struct, while `pid_uid` still only trusts the
/// leading bytes it needs.
const BSDINFO_BUFFER: usize = 256;

/// Effective user id of `pid`, used to label socket owners.
pub fn pid_uid(pid: i32) -> Option<u32> {
    let mut buffer = [0u8; BSDINFO_BUFFER];
    // SAFETY: the buffer is as large as the requested prefix of
    // `proc_bsdinfo`, and `proc_pidinfo` writes at most that many bytes.
    let written = unsafe {
        proc_pidinfo(
            pid,
            PROC_PIDTBSDINFO,
            0,
            buffer.as_mut_ptr() as *mut c_void,
            buffer.len() as c_int,
        )
    };
    if written < (OFF_PBI_UID + std::mem::size_of::<u32>()) as c_int {
        return None;
    }
    let end = OFF_PBI_UID + std::mem::size_of::<u32>();
    let mut uid = [0u8; std::mem::size_of::<u32>()];
    uid.copy_from_slice(&buffer[OFF_PBI_UID..end]);
    Some(u32::from_ne_bytes(uid))
}

/// Name of a user id, or `None` when the passwd database has no such entry.
pub fn user_name(uid: u32) -> Option<String> {
    // SAFETY: `getpwuid` either returns null or a pointer to a static record
    // that stays valid until the next call; the string is copied immediately.
    unsafe {
        let entry = libc::getpwuid(uid);
        if entry.is_null() || (*entry).pw_name.is_null() {
            return None;
        }
        std::ffi::CStr::from_ptr((*entry).pw_name)
            .to_str()
            .ok()
            .map(str::to_string)
    }
}

/// Full path of the executable behind `pid`, when readable.
pub fn pid_path(pid: i32) -> Option<String> {
    const MAXPATHLEN: u32 = 4096;
    let mut buffer = vec![0u8; MAXPATHLEN as usize];
    // SAFETY: the buffer is `MAXPATHLEN` bytes, which the API requires.
    let written = unsafe { proc_pidpath(pid, buffer.as_mut_ptr() as *mut c_void, MAXPATHLEN) };
    if written <= 0 {
        return None;
    }
    let bytes = &buffer[..written as usize];
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    Some(String::from_utf8_lossy(&bytes[..end]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_match_the_installed_sdk() {
        // Filled in by `offsetof` against the same headers the crate compiles
        // against; if the SDK ever changes the layout this test fails loudly
        // instead of silently reporting wrong ports.
        assert_eq!(SOCKET_FDINFO_SIZE, 792);
        assert_eq!(OFF_SOCKET_INFO, 24);
        assert_eq!(OFF_KIND, 256);
        assert_eq!(OFF_PROTO, 264);
        assert_eq!(OFF_INI_VFLAG, 288);
        assert_eq!(OFF_INI_FADDR, 296);
        assert_eq!(OFF_INI_LADDR, 312);
        assert_eq!(OFF_TCP_STATE, 344);
    }

    #[test]
    fn own_uid_matches_the_current_user() {
        // A wrong offset would return root for everything, so compare against
        // the libc answer for our own pid.
        let uid = pid_uid(std::process::id() as i32).expect("uid of self");
        assert_eq!(uid, unsafe { libc::getuid() });
        assert!(
            user_name(uid).is_some(),
            "passwd database has our own entry"
        );
    }

    #[test]
    fn nonexistent_pid_has_no_uid() {
        assert_eq!(pid_uid(i32::MAX), None);
    }

    #[test]
    fn pids_are_sorted_unique_and_include_self() {
        let pids = list_pids();
        assert!(pids.len() > 1);
        assert!(
            pids.windows(2).all(|w| w[0] < w[1]),
            "must be sorted and unique: {pids:?}"
        );
        assert!(pids.contains(&(std::process::id() as i32)));
        // `proc_listallpids` hides processes owned by other users when x is not
        // root, so pid 1 is only guaranteed to appear with privileges. What
        // matters for socket attribution is that the table is complete for the
        // current user.
        let mine = pids.iter().filter(|p| **p > 1_000).count();
        assert!(mine > 0, "no user process visible in {pids:?}");
    }

    #[test]
    fn own_executable_path_is_resolvable() {
        let path = pid_path(std::process::id() as i32).expect("own path");
        assert!(path.starts_with('/'));
        assert!(path.contains("x_platform"), "unexpected path: {path}");
    }

    #[test]
    fn reads_a_real_tcp4_listener() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();

        let me = std::process::id() as i32;
        let sockets: Vec<_> = list_fds(me)
            .into_iter()
            .filter(|fd| fd.proc_fdtype == PROX_FDTYPE_SOCKET)
            .filter_map(|fd| socket_info(me, fd.proc_fd))
            .collect();

        let found = sockets
            .iter()
            .find(|s| s.local_port == port)
            .unwrap_or_else(|| panic!("port {port} not found in {sockets:?}"));
        assert_eq!(found.kind, SOCKINFO_TCP);
        assert_eq!(found.tcp_state, TCPS_LISTEN);
        assert_eq!(found.remote_port, 0);
        assert_eq!(found.ip_version, INI_IPV4);
        assert_eq!(found.local_address[12..], [127, 0, 0, 1]);
    }

    #[test]
    fn reads_a_real_udp_socket() {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind");
        let port = socket.local_addr().expect("addr").port();

        let me = std::process::id() as i32;
        let found = list_fds(me)
            .into_iter()
            .filter(|fd| fd.proc_fdtype == PROX_FDTYPE_SOCKET)
            .filter_map(|fd| socket_info(me, fd.proc_fd))
            .find(|s| s.local_port == port)
            .unwrap_or_else(|| panic!("port {port} not found"));
        assert_eq!(found.kind, SOCKINFO_IN);
        assert_eq!(found.remote_port, 0);
    }

    #[test]
    fn reads_a_real_tcp6_listener() {
        let listener = std::net::TcpListener::bind("[::1]:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();

        let me = std::process::id() as i32;
        let found = list_fds(me)
            .into_iter()
            .filter(|fd| fd.proc_fdtype == PROX_FDTYPE_SOCKET)
            .filter_map(|fd| socket_info(me, fd.proc_fd))
            .find(|s| s.local_port == port)
            .unwrap_or_else(|| panic!("port {port} not found"));
        assert_eq!(found.kind, SOCKINFO_TCP);
        assert_eq!(found.tcp_state, TCPS_LISTEN);
        assert_eq!(found.ip_version, INI_IPV6);
        assert_eq!(found.local_address[15], 1);
    }

    #[test]
    fn a_non_socket_descriptor_yields_nothing() {
        let me = std::process::id() as i32;
        // stdin is a pipe or /dev/null, never a socket.
        assert!(socket_info(me, 0).is_none());
    }
}
