//! Windows socket attribution through the IP helper API.
//!
//! `GetExtendedTcpTable` and `GetExtendedUdpTable` return every connection with
//! the owning pid attached, which is the same information macOS gets from
//! `libproc` and Linux from `/proc/net`.

use super::buffer::AlignedBuffer;
use crate::common::process_sysinfo::SysinfoProcess;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use windows_sys::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, GetExtendedUdpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCPROW_OWNER_PID,
    MIB_TCP_STATE_ESTAB, MIB_TCP_STATE_FIN_WAIT1, MIB_TCP_STATE_FIN_WAIT2, MIB_TCP_STATE_LAST_ACK,
    MIB_TCP_STATE_LISTEN, MIB_TCP_STATE_SYN_RCVD, MIB_TCP_STATE_SYN_SENT, MIB_TCP_STATE_TIME_WAIT,
    MIB_UDP6ROW_OWNER_PID, MIB_UDPROW_OWNER_PID, TCP_TABLE_OWNER_PID_ALL, UDP_TABLE_OWNER_PID,
};
use x_core::error::{Error, Result};
use x_core::port::{ConnectionState, KillPlan, PortInfo, PortListOptions, PortManager, Protocol};
use x_core::process::KillSignal;

const AF_INET: u32 = 2;
const AF_INET6: u32 = 23;

/// Reads Windows sockets.
#[derive(Debug, Default)]
pub struct WindowsPort;

impl WindowsPort {
    /// Create the adapter.
    pub fn new() -> Self {
        Self
    }

    /// Every TCP and UDP endpoint with its owning pid, from the IP helper API.
    pub fn list_raw(&self) -> Vec<RawSocket> {
        let mut sockets = Vec::new();
        sockets.extend(tcp_table(AF_INET));
        sockets.extend(tcp_table(AF_INET6));
        sockets.extend(udp_table(AF_INET));
        sockets.extend(udp_table(AF_INET6));
        sockets
    }
}

/// One endpoint as the IP helper API reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSocket {
    /// Owning pid, always present on Windows.
    pub pid: u32,
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

/// The fixed part of every table: a row count followed by a flexible array.
///
/// Each row has a different size, so the buffer is walked with explicit offsets
/// rather than a slice cast.
struct Table {
    buffer: AlignedBuffer,
    count: u32,
    row_size: usize,
    header_size: usize,
}

impl Table {
    /// Byte offset of row `index`, if the buffer holds it.
    fn row_offset(&self, index: u32) -> Option<usize> {
        let start = self.header_size + index as usize * self.row_size;
        (start + self.row_size <= self.buffer.len()).then_some(start)
    }
}

/// Read a table whose row size is known at compile time.
fn read_table(
    row_size: usize,
    fill: unsafe fn(*mut std::ffi::c_void, *mut u32, u32) -> u32,
    family: u32,
) -> Option<Table> {
    let mut size: u32 = 0;
    // SAFETY: both calls receive the buffer size the API writes back.
    let rc = unsafe { fill(std::ptr::null_mut(), &mut size, family) };
    if size == 0 {
        return None;
    }
    // The size query is expected to fail with ERROR_INSUFFICIENT_BUFFER, so only
    // the value it wrote back matters here.
    let _ = rc;

    let mut buffer = AlignedBuffer::zeroed(size as usize);
    // SAFETY: `buffer` is at least the number of bytes the API asked for, and is
    // aligned for the structures the API writes into it.
    let rc = unsafe { fill(buffer.as_mut_ptr(), &mut size, family) };
    if rc != 0 {
        return None;
    }

    Some(Table {
        count: buffer.count(),
        buffer,
        row_size,
        header_size: 4,
    })
}

/// Read the TCP table for one address family.
fn tcp_table(family: u32) -> Vec<RawSocket> {
    if family == AF_INET {
        let table = tcp_table_v4();
        let Some(table) = table else {
            return Vec::new();
        };
        return (0..table.count)
            .filter_map(|index| table.row_offset(index))
            .map(|offset| {
                // SAFETY: the offset is inside the buffer and the table row size
                // is exactly one `MIB_TCPROW_OWNER_PID`. Rows are not guaranteed
                // to be aligned, so the read must not assume it.
                let raw = unsafe { table.buffer.read_at::<MIB_TCPROW_OWNER_PID>(offset) };
                RawSocket {
                    pid: raw.dwOwningPid,
                    protocol: Protocol::Tcp,
                    local_address: Ipv4Addr::from(raw.dwLocalAddr.to_le_bytes()).into(),
                    local_port: port(raw.dwLocalPort),
                    remote_address: Some(Ipv4Addr::from(raw.dwRemoteAddr.to_le_bytes()).into()),
                    remote_port: Some(port(raw.dwRemotePort)),
                    state: tcp_state(raw.dwState),
                }
            })
            .collect();
    }

    let Some(table) = tcp_table_v6() else {
        return Vec::new();
    };
    (0..table.count)
        .filter_map(|index| table.row_offset(index))
        .map(|offset| {
            // SAFETY: as above, for `MIB_TCP6ROW_OWNER_PID`.
            let raw = unsafe { table.buffer.read_at::<MIB_TCP6ROW_OWNER_PID>(offset) };
            RawSocket {
                pid: raw.dwOwningPid,
                protocol: Protocol::Tcp,
                local_address: Ipv6Addr::from(raw.ucLocalAddr).into(),
                local_port: port(raw.dwLocalPort),
                remote_address: Some(Ipv6Addr::from(raw.ucRemoteAddr).into()),
                remote_port: Some(port(raw.dwRemotePort)),
                state: tcp_state(raw.dwState),
            }
        })
        .collect()
}

fn tcp_table_v4() -> Option<Table> {
    // SAFETY: the function pointer matches GetExtendedTcpTable's signature.
    read_table(
        std::mem::size_of::<MIB_TCPROW_OWNER_PID>(),
        |buffer, size, family| unsafe {
            GetExtendedTcpTable(buffer, size, 0, family, TCP_TABLE_OWNER_PID_ALL, 0)
        },
        AF_INET,
    )
}

fn tcp_table_v6() -> Option<Table> {
    read_table(
        std::mem::size_of::<MIB_TCP6ROW_OWNER_PID>(),
        |buffer, size, family| unsafe {
            GetExtendedTcpTable(buffer, size, 0, family, TCP_TABLE_OWNER_PID_ALL, 0)
        },
        AF_INET6,
    )
}

/// Read the UDP table for one address family.
fn udp_table(family: u32) -> Vec<RawSocket> {
    if family == AF_INET {
        let Some(table) = read_table(
            std::mem::size_of::<MIB_UDPROW_OWNER_PID>(),
            |buffer, size, family| unsafe {
                GetExtendedUdpTable(buffer, size, 0, family, UDP_TABLE_OWNER_PID, 0)
            },
            AF_INET,
        ) else {
            return Vec::new();
        };
        return (0..table.count)
            .filter_map(|index| table.row_offset(index))
            .map(|offset| {
                // SAFETY: as above, for `MIB_UDPROW_OWNER_PID`.
                let raw = unsafe { table.buffer.read_at::<MIB_UDPROW_OWNER_PID>(offset) };
                RawSocket {
                    pid: raw.dwOwningPid,
                    protocol: Protocol::Udp,
                    local_address: Ipv4Addr::from(raw.dwLocalAddr.to_le_bytes()).into(),
                    local_port: port(raw.dwLocalPort),
                    // The UDP table has no peer; `dwLocalAddr` of 0.0.0.0 with a
                    // zero port is a row Windows cannot attribute, so keep it but
                    // report the state as bound.
                    remote_address: None,
                    remote_port: None,
                    state: ConnectionState::Bound,
                }
            })
            .collect();
    }

    let Some(table) = read_table(
        std::mem::size_of::<MIB_UDP6ROW_OWNER_PID>(),
        |buffer, size, family| unsafe {
            GetExtendedUdpTable(buffer, size, 0, family, UDP_TABLE_OWNER_PID, 0)
        },
        AF_INET6,
    ) else {
        return Vec::new();
    };
    (0..table.count)
        .filter_map(|index| table.row_offset(index))
        .map(|offset| {
            // SAFETY: as above, for `MIB_UDP6ROW_OWNER_PID`.
            let raw = unsafe { table.buffer.read_at::<MIB_UDP6ROW_OWNER_PID>(offset) };
            RawSocket {
                pid: raw.dwOwningPid,
                protocol: Protocol::Udp,
                local_address: Ipv6Addr::from(raw.ucLocalAddr).into(),
                local_port: port(raw.dwLocalPort),
                remote_address: None,
                remote_port: None,
                state: ConnectionState::Bound,
            }
        })
        .collect()
}

/// Ports are stored in network byte order inside a `u32`.
fn port(raw: u32) -> u16 {
    (raw as u16).to_be()
}

/// Map a `MIB_TCP_STATE` onto the unified state.
///
/// The API stores the state in a `u32` but defines the constants as `i32`, so
/// the value is narrowed once here instead of at every call site.
pub fn tcp_state(raw: u32) -> ConnectionState {
    match raw as i32 {
        MIB_TCP_STATE_LISTEN => ConnectionState::Listen,
        MIB_TCP_STATE_SYN_SENT => ConnectionState::SynSent,
        MIB_TCP_STATE_SYN_RCVD => ConnectionState::SynReceived,
        MIB_TCP_STATE_ESTAB => ConnectionState::Established,
        MIB_TCP_STATE_FIN_WAIT1 => ConnectionState::FinWait1,
        MIB_TCP_STATE_FIN_WAIT2 => ConnectionState::FinWait2,
        MIB_TCP_STATE_TIME_WAIT => ConnectionState::TimeWait,
        MIB_TCP_STATE_LAST_ACK => ConnectionState::LastAck,
        9 => ConnectionState::Closing,
        _ => ConnectionState::Unknown,
    }
}

impl PortManager for WindowsPort {
    fn list(&self, options: &PortListOptions) -> Result<Vec<PortInfo>> {
        let sockets = self.list_raw();
        if sockets.is_empty() {
            return Err(Error::system(
                "the IP helper API returned no TCP or UDP table",
            ));
        }

        let processes = SysinfoProcess::new();
        let mut pids: Vec<u32> = sockets.iter().map(|s| s.pid).collect();
        pids.sort_unstable();
        pids.dedup();
        let names: HashMap<u32, String> = pids
            .iter()
            .filter_map(|pid| processes.name_for(*pid).map(|name| (*pid, name)))
            .collect();
        // Tokens are expensive to open, so resolve one account per distinct pid
        // rather than one per socket.
        let users: HashMap<u32, String> = pids
            .iter()
            .filter_map(|pid| super::account_name(*pid).map(|name| (*pid, name)))
            .collect();

        let rows = sockets
            .into_iter()
            .map(|socket| PortInfo {
                protocol: socket.protocol,
                local_address: socket.local_address,
                local_port: socket.local_port,
                remote_address: socket.remote_address,
                remote_port: socket.remote_port,
                state: socket.state,
                pid: Some(socket.pid),
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
    std::sync::Arc::new(WindowsPort::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ports_are_read_in_network_byte_order() {
        // 8080 is 0x1F90, stored as 0x901F0000 on the little endian wire layout.
        assert_eq!(port(0x901F_0000), 8080);
        assert_eq!(port(0x0000_0000), 0);
    }

    #[test]
    fn states_are_normalised() {
        assert_eq!(
            tcp_state(MIB_TCP_STATE_LISTEN as u32),
            ConnectionState::Listen
        );
        assert_eq!(
            tcp_state(MIB_TCP_STATE_ESTAB as u32),
            ConnectionState::Established
        );
        assert_eq!(
            tcp_state(MIB_TCP_STATE_TIME_WAIT as u32),
            ConnectionState::TimeWait
        );
        assert_eq!(tcp_state(0), ConnectionState::Unknown);
    }

    #[test]
    fn a_real_listener_is_attributed_to_this_process() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();

        let rows = WindowsPort::new()
            .list(&PortListOptions {
                listening_only: true,
                ..Default::default()
            })
            .expect("list");
        let ours = rows
            .iter()
            .find(|row| row.local_port == port)
            .unwrap_or_else(|| panic!("port {port} missing from {rows:?}"));
        assert_eq!(ours.protocol, Protocol::Tcp);
        assert_eq!(ours.pid, Some(std::process::id()));
        assert!(ours.state.is_listening());
    }

    #[test]
    fn a_bound_udp_socket_is_reported() {
        let socket = std::net::UdpSocket::bind("127.0.0.1:0").expect("bind");
        let port = socket.local_addr().expect("addr").port();
        let rows = WindowsPort::new()
            .list(&PortListOptions {
                protocol: Some(Protocol::Udp),
                ..Default::default()
            })
            .expect("list");
        assert!(rows.iter().any(|row| row.local_port == port));
    }
}
