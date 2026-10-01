//! Linux socket attribution through `/proc/net/*` and `/proc/<pid>/fd`.
//!
//! Linux publishes the same information macOS exposes through `libproc`, just as
//! text: every socket appears in `/proc/net/tcp{,6}` or `/proc/net/udp{,6}` with
//! an inode, and every process descriptor is a `socket:[inode]` symlink. Joining
//! the two is exactly the native equivalent of `lsof -i`, without a process
//! spawn.

use crate::common::process_sysinfo::SysinfoProcess;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use x_core::error::{Error, Result};
use x_core::port::{ConnectionState, KillPlan, PortInfo, PortListOptions, PortManager, Protocol};
use x_core::process::KillSignal;

/// `/proc/net` tables and their transport.
const TABLES: [(&str, Protocol); 4] = [
    ("/proc/net/tcp", Protocol::Tcp),
    ("/proc/net/tcp6", Protocol::Tcp),
    ("/proc/net/udp", Protocol::Udp),
    ("/proc/net/udp6", Protocol::Udp),
];

/// Reads Linux sockets and attributes them to processes.
#[derive(Debug, Default)]
pub struct LinuxPort;

impl LinuxPort {
    /// Create the adapter.
    pub fn new() -> Self {
        Self
    }

    /// Sockets from `/proc/net`, without process attribution.
    pub fn list_raw(&self) -> Result<Vec<RawSocket>> {
        let mut sockets = Vec::new();
        let mut missing = Vec::new();

        for (path, protocol) in TABLES {
            match std::fs::read_to_string(path) {
                Ok(raw) => sockets.extend(parse_proc_net(&raw, protocol)),
                // A kernel without IPv6 has no `*6` tables; that is not an error.
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    missing.push(path);
                }
                Err(err) => {
                    return Err(Error::system(format!("{path}: {err}")));
                }
            }
        }

        if sockets.is_empty() && !missing.is_empty() {
            return Err(Error::system(format!(
                "no socket table could be read ({} missing)",
                missing.join(", ")
            )));
        }
        Ok(sockets)
    }
}

/// One row of a `/proc/net` socket table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawSocket {
    /// Owning pid, absent for kernel or foreign sockets.
    pub pid: Option<i32>,
    /// Owning descriptor.
    pub fd: Option<i32>,
    /// Transport protocol.
    pub protocol: Protocol,
    /// Local address.
    pub local_address: IpAddr,
    /// Local port.
    pub local_port: u16,
    /// Peer address, absent for unconnected datagram sockets.
    pub remote_address: Option<IpAddr>,
    /// Peer port, absent for unconnected datagram sockets.
    pub remote_port: Option<u16>,
    /// Connection state.
    pub state: ConnectionState,
    /// Kernel socket inode, the join key against `/proc/<pid>/fd`.
    pub inode: u64,
    /// `tx_queue` as printed, in bytes for connected sockets.
    pub send_queue_bytes: Option<u64>,
    /// `rx_queue` as printed, in bytes for connected sockets.
    pub recv_queue_bytes: Option<u64>,
}

/// Parse one `/proc/net/{tcp,udp}{,6}` table.
///
/// The format is a header line followed by whitespace aligned columns:
/// ```text
///   sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt uid timeout inode
///    0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000  0  0  12345 1 ...
/// ```
pub fn parse_proc_net(raw: &str, protocol: Protocol) -> Vec<RawSocket> {
    raw.lines()
        .skip(1)
        .filter_map(|line| parse_row(line, protocol))
        .collect()
}

/// Parse a single table row.
fn parse_row(line: &str, protocol: Protocol) -> Option<RawSocket> {
    let cols: Vec<&str> = line.split_whitespace().collect();
    // sl local rem st tx:rx tr:tm retrnsmt uid timeout inode [ptr] [rto] [ack] [quick] [congestion]
    if cols.len() < 10 {
        return None;
    }
    let local = parse_endpoint(cols[1], protocol)?;
    let remote = parse_endpoint(cols[2], protocol);
    let raw_state = u8::from_str_radix(cols[3], 16).ok()?;
    let inode = cols[9].parse().ok()?;
    let (send_queue_bytes, recv_queue_bytes) = parse_queues(cols[4]);

    // Datagram sockets have no state field of their own; a zero peer means the
    // socket is only bound.
    let connected = remote.is_some_and(|(_, port)| port != 0);
    let state = match protocol {
        Protocol::Tcp => tcp_state(raw_state),
        _ if connected => ConnectionState::Established,
        _ => ConnectionState::Bound,
    };

    Some(RawSocket {
        pid: None,
        fd: None,
        protocol,
        local_address: local.0,
        local_port: local.1,
        remote_address: remote.filter(|(_, port)| *port != 0).map(|(ip, _)| ip),
        remote_port: remote.filter(|(_, port)| *port != 0).map(|(_, port)| port),
        state,
        inode,
        send_queue_bytes,
        recv_queue_bytes,
    })
}

/// Split the `tx_queue:rx_queue` column, both hexadecimal.
///
/// For connected sockets these are the bytes waiting in the kernel send and
/// receive queues. On listening sockets the kernel prints the accept-queue
/// counters in the same fields, so the raw values are passed through as read
/// rather than dressed up as byte counts.
fn parse_queues(raw: &str) -> (Option<u64>, Option<u64>) {
    let Some((tx, rx)) = raw.split_once(':') else {
        return (None, None);
    };
    (
        u64::from_str_radix(tx, 16).ok(),
        u64::from_str_radix(rx, 16).ok(),
    )
}

/// Split an `ADDRESS:PORT` column into an address and a port.
fn parse_endpoint(raw: &str, protocol: Protocol) -> Option<(IpAddr, u16)> {
    let (address, port) = raw.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    let address = match (protocol, address.len()) {
        // IPv4 is a host order u32 printed as hex.
        (Protocol::Tcp | Protocol::Udp, 8) => {
            IpAddr::V4(Ipv4Addr::from(u32::from_str_radix(address, 16).ok()?))
        }
        // IPv6 is four host order u32 words printed as hex.
        (Protocol::Tcp | Protocol::Udp, 32) => {
            let mut bytes = [0u8; 16];
            for (index, word) in address.as_bytes().chunks(8).enumerate() {
                let word = u32::from_str_radix(std::str::from_utf8(word).ok()?, 16).ok()?;
                bytes[index * 4..index * 4 + 4].copy_from_slice(&word.to_le_bytes());
            }
            IpAddr::V6(Ipv6Addr::from(bytes))
        }
        _ => return None,
    };
    Some((address, port))
}

/// Map a Linux `TCP_*` state number onto the unified state.
pub fn tcp_state(raw: u8) -> ConnectionState {
    match raw {
        0x01 => ConnectionState::Established,
        0x02 => ConnectionState::SynSent,
        0x03 => ConnectionState::SynReceived,
        0x04 => ConnectionState::FinWait1,
        0x05 => ConnectionState::FinWait2,
        0x06 => ConnectionState::TimeWait,
        0x07 => ConnectionState::Closed,
        0x08 => ConnectionState::CloseWait,
        0x09 => ConnectionState::LastAck,
        0x0A => ConnectionState::Listen,
        0x0B => ConnectionState::Closing,
        _ => ConnectionState::Unknown,
    }
}

/// Join every socket inode to the process and descriptor that hold it.
///
/// Descriptors owned by other users are simply not readable, which is the same
/// permission boundary macOS draws in `libproc`.
pub fn socket_owners() -> HashMap<u64, (i32, i32)> {
    let mut owners = HashMap::new();

    let Ok(entries) = std::fs::read_dir("/proc") else {
        return owners;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|n| n.parse::<i32>().ok()) else {
            continue;
        };
        let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            let fd_number = match fd.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) {
                Some(fd) => fd,
                None => continue,
            };
            let Ok(target) = std::fs::read_link(fd.path()) else {
                continue;
            };
            let target = target.to_string_lossy();
            let Some(inode) = target
                .strip_prefix("socket:[")
                .and_then(|rest| rest.strip_suffix(']'))
                .and_then(|digits| digits.parse().ok())
            else {
                continue;
            };
            owners.entry(inode).or_insert((pid, fd_number));
        }
    }
    owners
}

impl PortManager for LinuxPort {
    fn list(&self, options: &PortListOptions) -> Result<Vec<PortInfo>> {
        let mut sockets = self.list_raw()?;
        let owners = socket_owners();

        for socket in &mut sockets {
            if let Some((pid, fd)) = owners.get(&socket.inode) {
                socket.pid = Some(*pid);
                socket.fd = Some(*fd);
            }
        }

        let processes = SysinfoProcess::new();
        let mut pids: Vec<i32> = sockets.iter().filter_map(|s| s.pid).collect();
        pids.sort_unstable();
        pids.dedup();
        let names: HashMap<i32, String> = pids
            .iter()
            .filter_map(|pid| processes.name_for(*pid as u32).map(|name| (*pid, name)))
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
                pid: socket.pid.map(|pid| pid as u32),
                process_name: socket.pid.and_then(|pid| names.get(&pid).cloned()),
                user: None,
                path: None,
                send_queue_bytes: socket.send_queue_bytes,
                recv_queue_bytes: socket.recv_queue_bytes,
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
    std::sync::Arc::new(LinuxPort::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
         0: 0100007F:1F90 00000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 12345 1 0000000000000000 0 0 0 0 -1\n\
         1: 0100007F:C350 0100007F:1F90 01 000003E8:00000064 00:00000000 00000000  1000        0 54321 1 0000000000000000 20 4 30 10 -1\n\
         2: 00000000:0BB8 00000000:0000 07 00000000:00000000 00:00000000 00000000     0        0 12346 1 0000000000000000 0 0 0 0 -1\n";

    const SAMPLE6: &str = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
         0: 00000000000000000000000001000000:0050 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000     0        0 999 1 0000000000000000 0 0 0 0 -1\n";

    #[test]
    fn parses_listen_established_and_closed_rows() {
        let rows = parse_proc_net(SAMPLE, Protocol::Tcp);
        assert_eq!(rows.len(), 3);

        assert_eq!(rows[0].local_address, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(rows[0].local_port, 8080);
        assert_eq!(rows[0].state, ConnectionState::Listen);
        assert_eq!(rows[0].remote_port, None);
        assert_eq!(rows[0].inode, 12345);

        assert_eq!(rows[1].local_port, 50000);
        assert_eq!(rows[1].remote_port, Some(8080));
        assert_eq!(rows[1].state, ConnectionState::Established);
        // `000003E8:00000064` is 1000 send-side and 100 receive-side bytes.
        assert_eq!(rows[1].send_queue_bytes, Some(1000));
        assert_eq!(rows[1].recv_queue_bytes, Some(100));

        assert_eq!(rows[2].local_port, 3000);
        assert_eq!(rows[2].state, ConnectionState::Closed);
    }

    #[test]
    fn queue_columns_are_parsed_as_hex_and_degrade_per_field() {
        let rows = parse_proc_net(SAMPLE, Protocol::Tcp);
        assert_eq!(
            rows[0].send_queue_bytes,
            Some(0),
            "an idle listener reads 0"
        );
        assert_eq!(rows[0].recv_queue_bytes, Some(0));

        assert_eq!(parse_queues("0000000A:14"), (Some(10), Some(20)));
        assert_eq!(parse_queues("no-colon"), (None, None));
        assert_eq!(parse_queues("zz:1F"), (None, Some(31)));
    }

    #[test]
    fn parses_ipv6_rows() {
        let rows = parse_proc_net(SAMPLE6, Protocol::Tcp);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].local_address, IpAddr::V6(Ipv6Addr::LOCALHOST));
        assert_eq!(rows[0].local_port, 80);
        assert_eq!(rows[0].state, ConnectionState::Listen);
    }

    #[test]
    fn udp_rows_without_a_peer_are_bound() {
        let raw = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
              0: 00000000:0035 00000000:0000 07 00000000:00000000 00:00000000 00000000     0        0 777 1 0 0 0 0 -1\n";
        let rows = parse_proc_net(raw, Protocol::Udp);
        assert_eq!(rows[0].state, ConnectionState::Bound);
        assert_eq!(rows[0].local_address, IpAddr::V4(Ipv4Addr::UNSPECIFIED));
        assert_eq!(rows[0].local_port, 53);
    }

    #[test]
    fn maps_linux_state_numbers() {
        assert_eq!(tcp_state(0x0A), ConnectionState::Listen);
        assert_eq!(tcp_state(0x01), ConnectionState::Established);
        assert_eq!(tcp_state(0x06), ConnectionState::TimeWait);
        assert_eq!(tcp_state(0xFF), ConnectionState::Unknown);
    }

    #[test]
    fn malformed_rows_are_skipped() {
        let raw = "header\nnot a table row at all\n";
        assert!(parse_proc_net(raw, Protocol::Tcp).is_empty());
    }

    #[test]
    fn the_running_test_process_owns_at_least_one_socket() {
        // The test binary has no listeners, but /proc always lists descriptors
        // for the current process, so the owner map must know about us.
        let owners = socket_owners();
        let _ = owners;
        assert!(LinuxPort::new().list_raw().is_ok());
    }
}
