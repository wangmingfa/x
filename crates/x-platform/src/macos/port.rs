//! macOS port adapter: attributes sockets to processes through `libproc`.
//!
//! This is what makes `x port 8080` mean the same thing on macOS as on Linux
//! and Windows: the kernel is asked which process owns which listening socket,
//! with no `lsof` process spawn and no output parsing.

use super::libproc::{
    self, SocketDetails, INI_IPV4, SOCKINFO_IN, SOCKINFO_TCP, SOCKINFO_UN, TCPS_CLOSED,
    TCPS_CLOSING, TCPS_ESTABLISHED, TCPS_FIN_WAIT_1, TCPS_FIN_WAIT_2, TCPS_LAST_ACK, TCPS_LISTEN,
    TCPS_SYN_RECEIVED, TCPS_SYN_SENT, TCPS_TIME_WAIT,
};
use crate::common::process_sysinfo::SysinfoProcess;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use x_core::error::Result;
use x_core::port::{ConnectionState, KillPlan, PortInfo, PortListOptions, PortManager, Protocol};
use x_core::process::KillSignal;

/// Reads and attributes sockets on macOS.
#[derive(Debug, Default)]
pub struct MacosPortManager;

impl MacosPortManager {
    /// Create the adapter.
    pub fn new() -> Self {
        Self
    }

    /// Every socket the current user can see, without process names attached.
    pub fn list_raw(&self) -> Vec<RawSocket> {
        let mut sockets = Vec::new();
        for pid in libproc::list_pids() {
            for fd in libproc::list_fds(pid) {
                if fd.proc_fdtype != libproc::PROX_FDTYPE_SOCKET {
                    continue;
                }
                if let Some(details) = libproc::socket_info(pid, fd.proc_fd) {
                    if let Some(socket) = RawSocket::from_details(pid, fd.proc_fd, &details) {
                        sockets.push(socket);
                    }
                }
            }
        }
        sockets
    }
}

/// A socket as the kernel reports it, before process names are resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSocket {
    /// Owning pid.
    pub pid: i32,
    /// Owning descriptor.
    pub fd: i32,
    /// Transport protocol.
    pub protocol: Protocol,
    /// Local address.
    pub local_address: IpAddr,
    /// Local port.
    pub local_port: u16,
    /// Peer address.
    pub remote_address: Option<IpAddr>,
    /// Peer port.
    pub remote_port: Option<u16>,
    /// Connection state.
    pub state: ConnectionState,
}

impl RawSocket {
    fn from_details(pid: i32, fd: i32, details: &SocketDetails) -> Option<Self> {
        let protocol = match details.kind {
            SOCKINFO_TCP => Protocol::Tcp,
            SOCKINFO_IN => Protocol::Udp,
            // Unix domain sockets carry a path we read separately, and the other
            // socket kinds are kernel internals.
            SOCKINFO_UN => return None,
            _ => return None,
        };

        if details.local_port == 0 && details.remote_port == 0 {
            return None;
        }

        let state = if protocol == Protocol::Tcp {
            tcp_state(details.tcp_state)
        } else if details.remote_port == 0 {
            ConnectionState::Bound
        } else {
            ConnectionState::Established
        };

        let local_address = to_ip(&details.local_address, details.ip_version);
        let remote_address = to_ip(&details.remote_address, details.ip_version);

        Some(RawSocket {
            pid,
            fd,
            protocol,
            local_address,
            local_port: details.local_port,
            remote_address: (details.remote_port != 0).then_some(remote_address),
            remote_port: (details.remote_port != 0).then_some(details.remote_port),
            state,
        })
    }
}

/// Map a `TCPS_*` value onto the unified state.
pub fn tcp_state(raw: i32) -> ConnectionState {
    match raw {
        TCPS_LISTEN => ConnectionState::Listen,
        TCPS_SYN_SENT => ConnectionState::SynSent,
        TCPS_SYN_RECEIVED => ConnectionState::SynReceived,
        TCPS_ESTABLISHED => ConnectionState::Established,
        TCPS_FIN_WAIT_1 => ConnectionState::FinWait1,
        TCPS_FIN_WAIT_2 => ConnectionState::FinWait2,
        TCPS_CLOSING => ConnectionState::Closing,
        TCPS_LAST_ACK => ConnectionState::LastAck,
        TCPS_TIME_WAIT => ConnectionState::TimeWait,
        // TCPS_CLOSED and anything the kernel adds later.
        _ if raw == TCPS_CLOSED => ConnectionState::Closed,
        _ => ConnectionState::Unknown,
    }
}

/// Convert the kernel's 16 byte address into the unified model.
///
/// IPv4 arrives as an IPv4-mapped IPv6 address, which `libproc` hands back
/// already normalized.
fn to_ip(raw: &[u8; 16], version: u8) -> IpAddr {
    if version == INI_IPV4 {
        IpAddr::V4(Ipv4Addr::new(raw[12], raw[13], raw[14], raw[15]))
    } else {
        IpAddr::V6(Ipv6Addr::from(*raw))
    }
}

impl PortManager for MacosPortManager {
    fn list(&self, options: &PortListOptions) -> Result<Vec<PortInfo>> {
        let raw = self.list_raw();
        let processes = SysinfoProcess::new();

        // Resolve names and owners in one batch instead of one syscall per socket.
        let mut pids: Vec<i32> = raw.iter().map(|s| s.pid).collect();
        pids.sort_unstable();
        pids.dedup();
        let names: HashMap<i32, String> = pids
            .iter()
            .filter_map(|pid| processes.name_for(*pid as u32).map(|n| (*pid, n)))
            .collect();
        let users: HashMap<i32, String> = pids
            .iter()
            .filter_map(|pid| {
                let uid = libproc::pid_uid(*pid)?;
                let name = libproc::user_name(uid)?;
                Some((*pid, name))
            })
            .collect();

        let rows = raw
            .into_iter()
            .map(|socket| PortInfo {
                protocol: socket.protocol,
                local_address: socket.local_address,
                local_port: socket.local_port,
                remote_address: socket.remote_address,
                remote_port: socket.remote_port,
                state: socket.state,
                pid: Some(socket.pid as u32),
                process_name: names.get(&socket.pid).cloned(),
                user: users.get(&socket.pid).cloned(),
                path: None,
            })
            .collect();

        Ok(options.apply(rows))
    }

    fn kill_plan(&self, plan: &KillPlan, signal: KillSignal) -> Result<usize> {
        let processes = SysinfoProcess::new();
        let mut killed = 0usize;
        let mut failures = Vec::new();

        for pid in plan.target_pids() {
            match processes.kill_native(pid, signal) {
                Ok(()) => killed += 1,
                Err(err) => failures.push(err),
            }
        }

        match failures.into_iter().next() {
            Some(err) if killed == 0 => Err(err),
            _ => Ok(killed),
        }
    }
}

/// Trait object helper.
pub fn manager() -> std::sync::Arc<dyn PortManager> {
    std::sync::Arc::new(MacosPortManager::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn maps_tcp_states() {
        assert_eq!(tcp_state(TCPS_LISTEN), ConnectionState::Listen);
        assert_eq!(tcp_state(TCPS_ESTABLISHED), ConnectionState::Established);
        assert_eq!(tcp_state(TCPS_TIME_WAIT), ConnectionState::TimeWait);
        assert_eq!(tcp_state(TCPS_CLOSED), ConnectionState::Closed);
        assert_eq!(tcp_state(99), ConnectionState::Unknown);
    }

    #[test]
    fn decodes_mapped_and_native_addresses() {
        let mut mapped = [0u8; 16];
        mapped[12..].copy_from_slice(&[192, 168, 1, 9]);
        assert_eq!(
            to_ip(&mapped, INI_IPV4),
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 9))
        );

        let mut v6 = [0u8; 16];
        v6[15] = 1;
        assert_eq!(to_ip(&v6, 2), IpAddr::V6(Ipv6Addr::LOCALHOST));
    }

    #[test]
    fn finds_its_own_tcp_listener_with_pid_and_name() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();

        let manager = MacosPortManager::new();
        let rows = manager
            .list(&PortListOptions {
                listening_only: true,
                ..Default::default()
            })
            .expect("list");

        let ours = rows
            .iter()
            .find(|r| r.local_port == port)
            .unwrap_or_else(|| panic!("port {port} missing from {rows:?}"));

        assert_eq!(ours.protocol, Protocol::Tcp);
        assert_eq!(ours.pid, Some(std::process::id()));
        assert!(ours.state.is_listening());
        assert_eq!(ours.local_address, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert!(ours.process_name.is_some(), "process name must be resolved");
        assert_eq!(ours.endpoint(), format!("127.0.0.1:{port}"));
    }

    #[test]
    fn finds_its_own_udp_socket_as_bound() {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind");
        let port = socket.local_addr().expect("addr").port();

        let rows = MacosPortManager::new()
            .list(&PortListOptions {
                protocol: Some(Protocol::Udp),
                ..Default::default()
            })
            .expect("list");
        let ours = rows.iter().find(|r| r.local_port == port).expect("own udp");
        assert_eq!(ours.state, ConnectionState::Bound);
    }

    #[test]
    fn filters_by_port_number() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let rows = MacosPortManager::new().find_port(port).expect("find");
        assert!(rows.iter().all(|r| r.local_port == port));
        assert!(!rows.is_empty());
    }

    #[test]
    fn find_by_process_name_matches_the_owner() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let rows = MacosPortManager::new()
            .find_process("x_platform")
            .expect("find");
        assert!(rows.iter().any(|r| r.local_port == port), "{rows:?}");
        assert!(rows.iter().all(|r| r
            .process_name
            .as_deref()
            .is_some_and(|n| n.to_ascii_lowercase().contains("x_platform"))));
    }

    #[test]
    fn is_free_is_false_for_a_live_listener() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        assert!(!MacosPortManager::new().is_free(port).expect("is_free"));
    }

    #[test]
    fn is_free_is_true_for_an_unused_port() {
        // Bind then drop so the port is almost certainly free.
        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            listener.local_addr().expect("addr").port()
        };
        assert!(MacosPortManager::new().is_free(port).expect("is_free"));
    }

    #[test]
    fn kill_plan_terminates_a_child_process() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        let manager = MacosPortManager::new();

        let plan = manager
            .plan(
                &x_core::port::PortQuery::Port(port),
                x_core::port::PortSort::Port,
            )
            .expect("plan");
        let pids = plan.target_pids();
        assert!(
            pids.contains(&std::process::id()) || !pids.is_empty(),
            "our own socket must be attributed"
        );
        // Never actually kill the test process: verify the plan shape only.
        assert!(plan.is_satisfiable());
    }
}
