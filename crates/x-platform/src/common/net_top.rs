//! Platform assembly for `x net top`: one sampling round into a
//! [`NetSnapshot`], whatever layers this OS can provide.
//!
//! L1 (host rate) and L2 (connection ownership) reuse the same managers the
//! rest of `x` reads, so they need no platform code here. L3 (per-process
//! bytes) is the only layer with a platform-specific source: packet capture
//! on Unix, absent elsewhere. A platform without it degrades to the two
//! lower layers rather than failing, and never estimates a rate.
//!
//! The capture device drains *interval* bytes per round, while `x-core`'s
//! [`x_core::net_top::diff`] expects *cumulative* counters (it subtracts the
//! previous snapshot from the current one, so a reset reads as a failure
//! rather than a negative rate). [`CaptureState`] is the seam where those
//! two semantics meet: drained bytes are folded into running totals, and the
//! totals go into the snapshot.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use x_core::net_top::{ConnectionOwner, InterfaceCounters, NetSnapshot};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use x_core::net_top::{PidBytes, SOURCE_SENTINEL};
use x_core::port::{PortListOptions, Protocol};
use x_core::SystemContext;

#[cfg(any(target_os = "macos", target_os = "linux"))]
use crate::common::capture::SocketEntry;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use crate::common::packet::{PROTO_TCP, PROTO_UDP};

/// The per-process byte layer: the capture sampler plus the running totals
/// that turn drained interval bytes into cumulative counters.
#[cfg(any(target_os = "macos", target_os = "linux"))]
struct CaptureState {
    sampler: crate::common::capture_unix::UnixCaptureSampler,
    running: Mutex<BTreeMap<i32, PidBytes>>,
}

/// The platform sampler for `x net top`. One instance per command run.
pub struct PlatformNetSampler {
    /// Why the per-process layer is absent, when it is: the capture open
    /// error on Unix, `None` when the layer is live or the platform never
    /// had one.
    per_process_error: Option<String>,
    /// Process names from the most recent socket-table read, keyed by pid:
    /// rows get labeled without a second process enumeration.
    names: Mutex<BTreeMap<i32, String>>,
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    capture: Option<CaptureState>,
}

impl Default for PlatformNetSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl PlatformNetSampler {
    /// Build the sampler, opening the capture device when the platform has
    /// one. A denied device is the expected case without root: L3 is then
    /// absent for the whole run and [`Self::per_process_available`] reports
    /// `false`, so the CLI can hint once instead of failing.
    pub fn new() -> Self {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            match crate::common::capture_unix::UnixCaptureSampler::open() {
                Ok(sampler) => Self {
                    per_process_error: None,
                    names: Mutex::new(BTreeMap::new()),
                    capture: Some(CaptureState {
                        sampler,
                        running: Mutex::new(BTreeMap::new()),
                    }),
                },
                Err(error) => Self {
                    per_process_error: Some(error.message().to_string()),
                    names: Mutex::new(BTreeMap::new()),
                    capture: None,
                },
            }
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            // No capture source is bound on this OS: host and connection
            // layers still run, so the command degrades instead of failing.
            Self {
                per_process_error: Some(
                    "per-process rates have no source on this platform yet".to_string(),
                ),
                names: Mutex::new(BTreeMap::new()),
            }
        }
    }

    /// Whether the per-process byte layer exists on this run. `false` means
    /// the platform has no source (or the device was denied), not that a
    /// read failed.
    pub fn per_process_available(&self) -> bool {
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            self.capture.is_some()
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            false
        }
    }

    /// Why the per-process layer is absent, when it is. The CLI surfaces
    /// this as a one-line hint instead of failing the command.
    pub fn per_process_error(&self) -> Option<&str> {
        self.per_process_error.as_deref()
    }

    /// Process name for `pid` from the most recent socket-table read.
    pub fn process_name(&self, pid: i32) -> Option<String> {
        self.names
            .lock()
            .expect("process names poisoned")
            .get(&pid)
            .cloned()
    }

    /// One sampling round: read the port table once (it feeds both the L2
    /// rows and the L3 attribution keys), capture for `window` against it,
    /// then assemble the snapshot.
    pub fn sample(&self, context: &SystemContext, window: Duration) -> NetSnapshot {
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let _ = window;
        let mut snapshot = NetSnapshot {
            interfaces: host_counters(context),
            ..NetSnapshot::default()
        };

        let rows = match context.port.list(&PortListOptions::default()) {
            Ok(rows) => rows,
            Err(error) => {
                snapshot
                    .failures
                    .push(("connections".into(), error.message().to_string()));
                return snapshot;
            }
        };
        {
            let mut names = self.names.lock().expect("process names poisoned");
            names.clear();
            for row in &rows {
                if let (Some(pid), Some(name)) = (row.pid, &row.process_name) {
                    if let Ok(pid) = i32::try_from(pid) {
                        names.insert(pid, name.clone());
                    }
                }
            }
        }
        snapshot.connections = Some(
            rows.iter()
                .filter_map(connection_owner)
                .collect::<Vec<ConnectionOwner>>(),
        );

        #[cfg(any(target_os = "macos", target_os = "linux"))]
        if let Some(capture) = &self.capture {
            let entries: Vec<SocketEntry> = rows.iter().filter_map(socket_entry).collect();
            match capture.sampler.sample(window, &|| entries.clone()) {
                Ok(captured) => {
                    let mut running = capture.running.lock().expect("capture totals poisoned");
                    if let Some(drained) = captured.pid_bytes {
                        for (pid, bytes) in drained {
                            let total = running.entry(pid).or_default();
                            total.rx += bytes.rx;
                            total.tx += bytes.tx;
                        }
                    }
                    // The sentinel tags the whole layer as capture-derived so
                    // the report says `port-inference`, never kernel accounting.
                    running.entry(SOURCE_SENTINEL).or_default();
                    snapshot.pid_bytes = Some(running.clone());
                }
                Err(error) => {
                    snapshot
                        .failures
                        .push(("per-process".into(), error.message().to_string()));
                }
            }
        }
        snapshot
    }
}

/// L1: cumulative byte counters per interface name. Interfaces that report
/// neither side are skipped; an all-empty host is indistinguishable from a
/// failed read and stays `None` either way.
fn host_counters(context: &SystemContext) -> Option<BTreeMap<String, InterfaceCounters>> {
    let interfaces = context.network.interfaces().ok()?;
    let mut counters = BTreeMap::new();
    for info in &interfaces {
        let (Some(rx), Some(tx)) = (info.received_bytes, info.transmitted_bytes) else {
            continue;
        };
        counters.insert(
            info.name.clone(),
            InterfaceCounters {
                rx_bytes: rx,
                tx_bytes: tx,
            },
        );
    }
    if counters.is_empty() {
        None
    } else {
        Some(counters)
    }
}

/// L2 row: transport sockets only — unix-domain rows are invisible to
/// capture and would only inflate the connection counts.
fn connection_owner(row: &x_core::PortInfo) -> Option<ConnectionOwner> {
    if !matches!(row.protocol, Protocol::Tcp | Protocol::Udp) {
        return None;
    }
    let remote = if row.state.is_listening() {
        // Listening sockets own the port but carry no peer.
        "-".to_string()
    } else {
        row.remote_socket_addr()
            .map_or_else(|| "-".to_string(), |address| address.to_string())
    };
    Some(ConnectionOwner {
        remote,
        pid: row.pid.and_then(|pid| i32::try_from(pid).ok()),
    })
}

/// L3 attribution key: the socket-table entry [`crate::common::capture::OwnerTable`]
/// matches packets against. Rows without an owner pid cannot receive credit.
#[cfg(any(target_os = "macos", target_os = "linux"))]
fn socket_entry(row: &x_core::PortInfo) -> Option<SocketEntry> {
    if !matches!(row.protocol, Protocol::Tcp | Protocol::Udp) {
        return None;
    }
    Some(SocketEntry {
        proto: if matches!(row.protocol, Protocol::Tcp) {
            PROTO_TCP
        } else {
            PROTO_UDP
        },
        local_ip: row.local_address,
        local_port: row.local_port,
        remote: row.remote_socket_addr().map(|a| (a.ip(), a.port())),
        pid: i32::try_from(row.pid?).ok()?,
    })
}

/// The `x net top` capability row, probed the only honest way: by actually
/// opening this platform's per-process source. It lives in `x-platform`
/// because the capture device is platform knowledge `x-core` must not have;
/// the CLI slots the row in with the rest of the `net` domain.
pub fn net_top_capability_rows() -> Vec<x_core::Capability> {
    let sampler = PlatformNetSampler::new();
    let (status, note) = if sampler.per_process_available() {
        (
            x_core::CapabilityStatus::Supported,
            Some("per-process rates via packet capture (port-inference)".to_string()),
        )
    } else {
        (
            x_core::CapabilityStatus::Degraded,
            Some(
                sampler
                    .per_process_error()
                    .unwrap_or("no per-process source on this platform")
                    .to_string(),
            ),
        )
    };
    vec![x_core::Capability {
        domain: "net".to_string(),
        feature: "net-top per-process rates".to_string(),
        status,
        note,
    }]
}

#[cfg(all(test, any(target_os = "macos", target_os = "linux")))]
mod tests {
    use super::*;

    fn row(protocol: Protocol, state: x_core::port::ConnectionState) -> x_core::PortInfo {
        use std::net::{IpAddr, Ipv4Addr};
        x_core::PortInfo {
            protocol,
            local_address: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5)),
            local_port: 51000,
            remote_address: Some(IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))),
            remote_port: Some(443),
            state,
            pid: Some(4242),
            process_name: None,
            user: None,
            path: None,
            send_queue_bytes: None,
            recv_queue_bytes: None,
        }
    }

    #[test]
    fn transport_rows_convert_with_remote_and_pid() {
        let owner = connection_owner(&row(
            Protocol::Tcp,
            x_core::port::ConnectionState::Established,
        ))
        .expect("tcp row converts");
        assert_eq!(owner.pid, Some(4242));
        assert_eq!(owner.remote, "93.184.216.34:443");

        let entry = socket_entry(&row(
            Protocol::Tcp,
            x_core::port::ConnectionState::Established,
        ))
        .expect("tcp row converts");
        assert_eq!(entry.proto, PROTO_TCP);
        assert_eq!(entry.local_port, 51000);
        assert_eq!(
            entry.remote,
            Some((
                std::net::IpAddr::V4(std::net::Ipv4Addr::new(93, 184, 216, 34)),
                443
            ))
        );
    }

    #[test]
    fn listening_rows_label_the_remote_as_none() {
        let owner =
            connection_owner(&row(Protocol::Udp, x_core::port::ConnectionState::Listen)).unwrap();
        assert_eq!(owner.remote, "-");
    }

    #[test]
    fn running_totals_fold_drained_interval_bytes() {
        // The seam's whole job: interval bytes in, cumulative counters out,
        // so diff()'s subtraction sees monotonic counters.
        let mut running: BTreeMap<i32, PidBytes> = BTreeMap::new();
        let drained = PidBytes { rx: 500, tx: 100 };
        let total = running.entry(42).or_default();
        total.rx += drained.rx;
        total.tx += drained.tx;
        let drained = PidBytes { rx: 300, tx: 50 };
        let total = running.entry(42).or_default();
        total.rx += drained.rx;
        total.tx += drained.tx;
        assert_eq!(running.get(&42), Some(&PidBytes { rx: 800, tx: 150 }));
    }
}
