//! Unified port / socket model.

use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// Transport protocol of a socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    /// TCP.
    Tcp,
    /// UDP.
    Udp,
    /// Unix domain socket.
    Unix,
    /// Anything else (raw ICMP, SCTP, ...).
    Other(u16),
}

impl Protocol {
    /// Canonical lowercase name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Tcp => "tcp",
            Self::Udp => "udp",
            Self::Unix => "unix",
            Self::Other(_) => "other",
        }
    }

    /// IANA protocol number when known.
    pub const fn number(self) -> u16 {
        match self {
            Self::Tcp => 6,
            Self::Udp => 17,
            Self::Unix => 0,
            Self::Other(n) => n,
        }
    }

    /// Parse from a CLI value.
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "tcp" => Some(Self::Tcp),
            "udp" => Some(Self::Udp),
            "unix" | "uds" => Some(Self::Unix),
            _ => None,
        }
    }

    /// `true` when the protocol is connection oriented.
    pub const fn is_stream(self) -> bool {
        matches!(self, Self::Tcp | Self::Unix)
    }
}

impl std::fmt::Display for Protocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// Connection state, normalized across platforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    /// Accepting connections: `LISTEN` on Linux, `LISTENING` on Windows,
    /// `TCPS_LISTEN` on macOS.
    Listen,
    /// Connection established.
    Established,
    /// Received `SYN`, waiting for the handshake reply.
    SynReceived,
    /// Connection attempt in progress.
    SynSent,
    /// Closed by the local side, waiting for the final acknowledgement.
    FinWait1,
    /// Both sides closed, still waiting for the final acknowledgement.
    FinWait2,
    /// Closed locally, waiting for the peer.
    CloseWait,
    /// Local side is closing.
    Closing,
    /// Peer closed, local side is waiting for the final acknowledgement.
    LastAck,
    /// Waiting to be reused.
    TimeWait,
    /// Closed on macOS.
    Closed,
    /// Datagram socket bound to an address, no peer.
    Bound,
    /// The platform does not report a state.
    Unknown,
}

impl ConnectionState {
    /// `true` for sockets that accept new connections.
    pub const fn is_listening(self) -> bool {
        matches!(self, Self::Listen)
    }

    /// `true` for sockets with an established peer.
    pub const fn is_connected(self) -> bool {
        matches!(self, Self::Established)
    }
}

/// A socket endpoint, listening or connected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortInfo {
    /// Transport protocol.
    pub protocol: Protocol,
    /// Local bound address.
    pub local_address: IpAddr,
    /// Local bound port.
    pub local_port: u16,
    /// Peer address for connected sockets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_address: Option<IpAddr>,
    /// Peer port for connected sockets.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_port: Option<u16>,
    /// Connection state.
    pub state: ConnectionState,
    /// Owning process id when the platform can attribute the socket.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Owning process name when the platform can attribute the socket.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process_name: Option<String>,
    /// Owning user when the platform can attribute the socket.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Unix domain socket path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

impl PortInfo {
    /// Local socket address.
    pub fn local_socket_addr(&self) -> SocketAddr {
        SocketAddr::new(self.local_address, self.local_port)
    }

    /// Remote socket address when the socket has a peer.
    pub fn remote_socket_addr(&self) -> Option<SocketAddr> {
        match (self.remote_address, self.remote_port) {
            (Some(addr), Some(port)) => Some(SocketAddr::new(addr, port)),
            _ => None,
        }
    }

    /// `true` when the socket is a listening socket.
    pub fn is_listening(&self) -> bool {
        self.state.is_listening()
    }

    /// Human friendly endpoint, e.g. `127.0.0.1:8080` or `0.0.0.0:8080`.
    pub fn endpoint(&self) -> String {
        match self.local_address {
            // Wildcard bindings are the common case: show the address, not a
            // bare colon that reads like a missing value.
            IpAddr::V4(ip) if ip == Ipv4Addr::UNSPECIFIED => {
                format!("0.0.0.0:{}", self.local_port)
            }
            IpAddr::V6(ip) if ip == Ipv6Addr::UNSPECIFIED => format!("[::]:{}", self.local_port),
            IpAddr::V4(ip) => format!("{ip}:{}", self.local_port),
            IpAddr::V6(ip) => format!("[{ip}]:{}", self.local_port),
        }
    }
}

/// Port query, either by number or by owning process name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PortQuery {
    /// Look up a specific port number.
    Port(u16),
    /// Look up every socket owned by processes matching a name.
    Process(String),
}

/// Filter and ordering options for port snapshots.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PortListOptions {
    /// Only this protocol.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocol: Option<Protocol>,
    /// Only sockets in this state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<ConnectionState>,
    /// Only listening sockets. Set by `x port list --listen`.
    pub listening_only: bool,
    /// Case-insensitive substring matched against port, process name and address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
    /// Maximum number of rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

impl PortListOptions {
    /// Listening sockets only.
    pub fn listening() -> Self {
        Self {
            listening_only: true,
            ..Default::default()
        }
    }

    /// Filter a snapshot and apply [`PortListOptions::limit`].
    ///
    /// Every adapter funnels through this helper so filters and truncation
    /// behave identically on Windows, Linux and macOS. Apply it *after* names
    /// are resolved, because [`PortListOptions::search`] also matches the
    /// process name.
    pub fn apply(&self, rows: Vec<PortInfo>) -> Vec<PortInfo> {
        let mut rows: Vec<PortInfo> = rows.into_iter().filter(|row| self.matches(row)).collect();
        if let Some(limit) = self.limit {
            rows.truncate(limit);
        }
        rows
    }

    /// `true` when `self` accepts `info` as a row.
    pub fn matches(&self, info: &PortInfo) -> bool {
        if let Some(proto) = self.protocol {
            if info.protocol != proto {
                return false;
            }
        }
        if let Some(state) = self.state {
            if info.state != state {
                return false;
            }
        }
        if self.listening_only && !info.is_listening() {
            return false;
        }
        if let Some(term) = self.search.as_deref() {
            let term = term.to_ascii_lowercase();
            let haystack = format!(
                "{} {} {} {}",
                info.local_port,
                info.process_name.as_deref().unwrap_or(""),
                info.local_address,
                info.pid.unwrap_or_default()
            )
            .to_ascii_lowercase();
            if !haystack.contains(&term) {
                return false;
            }
        }
        true
    }
}

/// The exact set of processes that would be terminated to satisfy a port query.
///
/// Produced by [`crate::port::PortManager::plan`] and consumed by both the CLI
/// (`x port kill`) and the TUI confirmation dialog, so a port is never killed
/// by a different code path than the one the user previewed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KillPlan {
    /// What the user asked for.
    pub query: PortQuery,
    /// Matching sockets.
    pub sockets: Vec<PortInfo>,
    /// Ordering requested by the frontend.
    #[serde(default)]
    pub sort: PortSort,
}

impl KillPlan {
    /// Build a plan from matching sockets, sorted for stable display.
    pub fn new(query: PortQuery, mut sockets: Vec<PortInfo>) -> Self {
        sockets.sort_by_key(|p| (p.local_port, p.protocol));
        Self {
            query,
            sockets,
            sort: PortSort::Port,
        }
    }

    /// Distinct process ids that own at least one matching socket.
    pub fn target_pids(&self) -> Vec<u32> {
        let mut pids: Vec<u32> = self.sockets.iter().filter_map(|p| p.pid).collect();
        pids.sort_unstable();
        pids.dedup();
        pids
    }

    /// Owners grouped per process, for rendering.
    pub fn owners(&self) -> Vec<PortOwner> {
        crate::port::manager::group_by_owner(self.sockets.clone())
    }

    /// `true` when no socket matched.
    pub fn is_empty(&self) -> bool {
        self.sockets.is_empty()
    }

    /// Whether the query was satisfiable at all.
    pub fn is_satisfiable(&self) -> bool {
        !self.sockets.is_empty()
    }

    /// Processes that cannot be killed without extra privileges, if any.
    pub fn elevated_pids(&self, current_user: Option<&str>) -> Vec<u32> {
        let Some(me) = current_user else {
            return Vec::new();
        };
        let mut pids: Vec<u32> = self
            .sockets
            .iter()
            .filter(|p| p.pid.is_some_and(|pid| pid != std::process::id()))
            .filter(|p| match p.user.as_deref() {
                Some(owner) => owner != me,
                None => false,
            })
            .filter_map(|p| p.pid)
            .collect();
        pids.sort_unstable();
        pids.dedup();
        pids
    }

    /// Privilege requirement for the whole plan.
    pub fn permission(&self, current_user: Option<&str>) -> crate::error::PermissionRequirement {
        if self.elevated_pids(current_user).is_empty() {
            crate::error::PermissionRequirement::None
        } else {
            crate::error::PermissionRequirement::Elevated
        }
    }
}

/// Sort keys supported by [`PortListOptions`] consumers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PortSort {
    /// Ascending port number. Default.
    #[default]
    Port,
    /// Process name, then port.
    Process,
    /// Pids first.
    Pid,
}

/// A process that owns one or more matching ports, with what would be killed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PortOwner {
    /// Owning process id.
    pub pid: u32,
    /// Owning process name.
    pub process_name: Option<String>,
    /// Ports owned by the process.
    pub ports: Vec<PortInfo>,
}

impl PortOwner {
    /// Ports only, in display order.
    pub fn port_numbers(&self) -> Vec<u16> {
        self.ports.iter().map(|p| p.local_port).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(port: u16, state: ConnectionState, name: Option<&str>) -> PortInfo {
        PortInfo {
            protocol: Protocol::Tcp,
            local_address: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            local_port: port,
            remote_address: None,
            remote_port: None,
            state,
            pid: Some(1234),
            process_name: name.map(str::to_string),
            user: None,
            path: None,
        }
    }

    #[test]
    fn endpoint_shows_wildcard_addresses_explicitly() {
        // A bare ":8080" reads like a missing value; both families spell the
        // wildcard out, matching the IPv6 branch below.
        assert_eq!(
            info(8080, ConnectionState::Listen, None).endpoint(),
            "0.0.0.0:8080"
        );
        let mut s = info(8080, ConnectionState::Listen, None);
        s.local_address = IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1));
        assert_eq!(s.endpoint(), "127.0.0.1:8080");
        s.local_address = IpAddr::V6(Ipv6Addr::UNSPECIFIED);
        assert_eq!(s.endpoint(), "[::]:8080");
    }

    #[test]
    fn listening_filter() {
        let opts = PortListOptions::listening();
        assert!(opts.matches(&info(80, ConnectionState::Listen, None)));
        assert!(!opts.matches(&info(80, ConnectionState::Established, None)));
    }

    #[test]
    fn search_matches_process_name() {
        let opts = PortListOptions {
            search: Some("node".into()),
            ..Default::default()
        };
        assert!(opts.matches(&info(3000, ConnectionState::Listen, Some("node"))));
        assert!(!opts.matches(&info(3000, ConnectionState::Listen, Some("nginx"))));
    }

    #[test]
    fn protocol_numbers() {
        assert_eq!(Protocol::Tcp.number(), 6);
        assert_eq!(Protocol::Udp.number(), 17);
        assert_eq!(Protocol::parse("TCP"), Some(Protocol::Tcp));
    }
}
