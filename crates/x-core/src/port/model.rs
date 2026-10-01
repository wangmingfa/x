//! Unified port / socket model.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
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
    /// Bytes sitting in the kernel send queue, when the platform reports it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_queue_bytes: Option<u64>,
    /// Bytes sitting in the kernel receive queue, when the platform reports it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recv_queue_bytes: Option<u64>,
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

/// The change set between two socket snapshots.
///
/// Produced by [`diff_sockets`] and consumed by `x port watch`, which prints
/// only what changed since the previous poll.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SocketDiff {
    /// Sockets present now but not before.
    pub added: Vec<PortInfo>,
    /// Sockets present before but not now.
    pub removed: Vec<PortInfo>,
}

impl SocketDiff {
    /// `true` when the two snapshots were identical.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }
}

/// Counts over one socket snapshot: by state, by protocol and queue
/// occupancy.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PortStats {
    /// Every socket in the snapshot.
    pub total: usize,
    /// Sockets per connection state, in the state enum's order.
    pub by_state: BTreeMap<ConnectionState, usize>,
    /// Sockets per protocol name (`tcp`, `udp`, `unix`, `other`).
    pub by_protocol: BTreeMap<String, usize>,
    /// Queue occupancy over the sockets whose platform reports it.
    pub queues: QueueStats,
}

/// Queue occupancy, counted per direction.
///
/// Platforms differ in what they expose: Linux prints both queues for every
/// socket, macOS only the TCP send buffer, Windows neither. Keeping the two
/// directions apart means "0 B" is never claimed for a queue no platform
/// reported.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueStats {
    /// Sockets that reported a send queue length.
    pub send_reporting: usize,
    /// Sockets that reported a receive queue length.
    pub recv_reporting: usize,
    /// Sum of reported send queue bytes.
    pub send_bytes: u64,
    /// Sum of reported receive queue bytes.
    pub recv_bytes: u64,
    /// Sockets with a non-empty queue in either reported direction.
    pub backed_up: usize,
}

impl QueueStats {
    /// `true` when no socket reported a queue length.
    pub fn is_empty(&self) -> bool {
        self.send_reporting == 0 && self.recv_reporting == 0
    }
}

/// Reduce a socket snapshot into [`PortStats`].
pub fn summarize(rows: &[PortInfo]) -> PortStats {
    let mut stats = PortStats {
        total: rows.len(),
        ..Default::default()
    };
    for row in rows {
        *stats.by_state.entry(row.state).or_default() += 1;
        *stats
            .by_protocol
            .entry(row.protocol.name().to_string())
            .or_default() += 1;
        if let Some(bytes) = row.send_queue_bytes {
            stats.queues.send_reporting += 1;
            stats.queues.send_bytes += bytes;
        }
        if let Some(bytes) = row.recv_queue_bytes {
            stats.queues.recv_reporting += 1;
            stats.queues.recv_bytes += bytes;
        }
        if row.send_queue_bytes.unwrap_or(0) > 0 || row.recv_queue_bytes.unwrap_or(0) > 0 {
            stats.queues.backed_up += 1;
        }
    }
    stats
}

/// Socket identity between snapshots: a state change or an ownership change is
/// reported as removal plus addition, which keeps the diff a plain set
/// difference. Name and user are derived from the pid, so they stay out.
type SocketKey = (
    Protocol,
    IpAddr,
    u16,
    Option<IpAddr>,
    Option<u16>,
    ConnectionState,
    Option<u32>,
);

fn socket_key(info: &PortInfo) -> SocketKey {
    (
        info.protocol,
        info.local_address,
        info.local_port,
        info.remote_address,
        info.remote_port,
        info.state,
        info.pid,
    )
}

/// Diff two socket snapshots, each side sorted by port and protocol.
pub fn diff_sockets(previous: &[PortInfo], current: &[PortInfo]) -> SocketDiff {
    use std::collections::BTreeSet;

    let before: BTreeSet<SocketKey> = previous.iter().map(socket_key).collect();
    let after: BTreeSet<SocketKey> = current.iter().map(socket_key).collect();

    let sort = |mut rows: Vec<PortInfo>| {
        rows.sort_by_key(|p| (p.local_port, p.protocol, p.local_address));
        rows
    };

    let mut seen = std::collections::BTreeSet::new();
    let added = sort(
        current
            .iter()
            .filter(|p| {
                let key = socket_key(p);
                !before.contains(&key) && seen.insert(key)
            })
            .cloned()
            .collect(),
    );
    let mut seen = std::collections::BTreeSet::new();
    let removed = sort(
        previous
            .iter()
            .filter(|p| {
                let key = socket_key(p);
                !after.contains(&key) && seen.insert(key)
            })
            .cloned()
            .collect(),
    );
    SocketDiff { added, removed }
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
            send_queue_bytes: None,
            recv_queue_bytes: None,
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

    #[test]
    fn diff_reports_added_and_removed_sockets() {
        let before = vec![
            info(80, ConnectionState::Listen, None),
            info(443, ConnectionState::Listen, None),
        ];
        let after = vec![
            info(443, ConnectionState::Listen, None),
            info(8080, ConnectionState::Listen, None),
        ];
        let diff = diff_sockets(&before, &after);
        assert_eq!(
            diff.added.iter().map(|p| p.local_port).collect::<Vec<_>>(),
            vec![8080]
        );
        assert_eq!(
            diff.removed
                .iter()
                .map(|p| p.local_port)
                .collect::<Vec<_>>(),
            vec![80]
        );
    }

    #[test]
    fn diff_treats_state_change_as_remove_plus_add() {
        let before = vec![info(22, ConnectionState::Listen, None)];
        let after = vec![info(22, ConnectionState::Established, None)];
        let diff = diff_sockets(&before, &after);
        assert_eq!(diff.added.len(), 1);
        assert_eq!(diff.removed.len(), 1);
        assert_eq!(diff.added[0].state, ConnectionState::Established);
        assert_eq!(diff.removed[0].state, ConnectionState::Listen);
    }

    #[test]
    fn diff_treats_ownership_change_as_remove_plus_add() {
        let mut before = info(3000, ConnectionState::Listen, None);
        before.pid = Some(1);
        let mut after = info(3000, ConnectionState::Listen, None);
        after.pid = Some(2);
        let diff = diff_sockets(&[before], &[after]);
        assert_eq!(diff.added[0].pid, Some(2));
        assert_eq!(diff.removed[0].pid, Some(1));
    }

    #[test]
    fn diff_of_identical_snapshots_is_empty() {
        let rows = vec![info(80, ConnectionState::Listen, None)];
        assert!(diff_sockets(&rows, &rows).is_empty());
    }

    #[test]
    fn summarize_counts_states_protocols_and_queues() {
        let mut idle = info(80, ConnectionState::Listen, None);
        idle.send_queue_bytes = Some(0);
        idle.recv_queue_bytes = Some(0);
        let mut sending = info(443, ConnectionState::Established, None);
        sending.protocol = Protocol::Udp;
        sending.send_queue_bytes = Some(2_000);
        let mut stale = info(8080, ConnectionState::TimeWait, None);
        stale.recv_queue_bytes = Some(500);

        let stats = summarize(&[idle, sending, stale]);
        assert_eq!(stats.total, 3);
        assert_eq!(stats.by_state.get(&ConnectionState::Listen), Some(&1));
        assert_eq!(stats.by_state.get(&ConnectionState::Established), Some(&1));
        assert_eq!(stats.by_state.get(&ConnectionState::TimeWait), Some(&1));
        assert_eq!(stats.by_protocol.get("tcp"), Some(&2));
        assert_eq!(stats.by_protocol.get("udp"), Some(&1));
        assert_eq!(stats.queues.send_reporting, 2);
        assert_eq!(stats.queues.recv_reporting, 2);
        assert_eq!(stats.queues.send_bytes, 2_000);
        assert_eq!(stats.queues.recv_bytes, 500);
        assert_eq!(stats.queues.backed_up, 2, "idle socket is not backed up");
    }

    #[test]
    fn summarize_without_queue_reporting_is_empty_not_zero() {
        let stats = summarize(&[info(80, ConnectionState::Listen, None)]);
        assert!(
            stats.queues.is_empty(),
            "no platform reported a queue, so nothing may claim 0 bytes"
        );
        assert_eq!(stats.queues.send_bytes, 0);
    }

    #[test]
    fn summarize_of_nothing_is_all_empty() {
        let stats = summarize(&[]);
        assert_eq!(stats.total, 0);
        assert!(stats.by_state.is_empty());
        assert!(stats.by_protocol.is_empty());
        assert!(stats.queues.is_empty());
    }

    #[test]
    fn stats_json_uses_readable_keys() {
        let stats = summarize(&[info(22, ConnectionState::TimeWait, None)]);
        let value: serde_json::Value = serde_json::to_value(&stats).expect("json");
        assert_eq!(value["by_state"]["time_wait"], 1);
        assert_eq!(value["by_protocol"]["tcp"], 1);
        assert_eq!(value["queues"]["send_reporting"], 0);
    }
}
