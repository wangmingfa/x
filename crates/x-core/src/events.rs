//! The system event stream: sampled snapshots and the diffs between them.
//!
//! There is no kernel subscription anywhere in this module, by design — the
//! workspace rule is "snapshot, never watch". [`SystemSnapshot::sample`]
//! reads the same managers every other command reads, and [`diff`] turns two
//! consecutive snapshots into [`SystemEvent`]s. The CLI and the TUI run the
//! same loop (sample → diff → render) on top of these pieces.
//!
//! A snapshot is per family: a family whose read fails lands in
//! [`SystemSnapshot::failures`] and its slot stays empty. The diff only
//! compares families that *both* sides sampled, so one flaky read cannot
//! manufacture a burst of fake start/stop events when it recovers.

use serde::Serialize;

use crate::context::SystemContext;
use crate::device::DeviceClass;
use crate::error::{Error, Result};
use crate::port::PortListOptions;
use crate::process::ProcessListOptions;
use crate::service::ServiceListOptions;

/// Which family a change belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    /// Processes appearing and disappearing.
    Process,
    /// Sockets being opened and closed.
    Connection,
    /// USB devices attached and detached (the `x device usb` set).
    Usb,
    /// Filesystems mounted and unmounted (the `x disk list` set).
    Mount,
    /// Services appearing, disappearing and changing state.
    Service,
}

impl EventType {
    /// Every family, in the order events are emitted.
    pub const ALL: [EventType; 5] = [
        EventType::Process,
        EventType::Connection,
        EventType::Usb,
        EventType::Mount,
        EventType::Service,
    ];

    /// Families sampled when the caller does not choose.
    ///
    /// USB is deliberately absent: its adapter shells out on Windows
    /// (`Get-PnpDevice`, measured ~1.6 s per call on a laptop), which is too
    /// heavy to poll. The other four read native APIs on every platform.
    pub const DEFAULT: [EventType; 4] = [
        EventType::Process,
        EventType::Connection,
        EventType::Mount,
        EventType::Service,
    ];

    /// Canonical lowercase token.
    pub const fn name(self) -> &'static str {
        match self {
            EventType::Process => "process",
            EventType::Connection => "connection",
            EventType::Usb => "usb",
            EventType::Mount => "mount",
            EventType::Service => "service",
        }
    }

    /// Parse a `--types` token.
    pub fn parse(token: &str) -> Option<Self> {
        match token.to_ascii_lowercase().as_str() {
            "process" => Some(EventType::Process),
            "connection" => Some(EventType::Connection),
            "usb" => Some(EventType::Usb),
            "mount" => Some(EventType::Mount),
            "service" => Some(EventType::Service),
            _ => None,
        }
    }
}

/// What happened to one thing between two snapshots.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EventOp {
    /// It appeared: a process started, a connection opened, …
    Added,
    /// It disappeared: a process stopped, a connection closed, …
    Removed,
    /// It stayed, but its state changed (services).
    Changed,
}

impl EventOp {
    /// Single character for the text stream.
    pub const fn symbol(self) -> char {
        match self {
            EventOp::Added => '+',
            EventOp::Removed => '-',
            EventOp::Changed => '~',
        }
    }
}

/// One change between two snapshots.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SystemEvent {
    /// Family the change belongs to.
    #[serde(rename = "type")]
    pub event_type: EventType,
    /// Appeared / disappeared / changed.
    pub op: EventOp,
    /// One human line, complete on its own.
    pub summary: String,
    /// Owning process id, when the change has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Process / device / service name, when the change has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Extra identifier: USB instance id, filesystem type, …
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

// ---------------------------------------------------------------------------
// The comparable records each family reduces to
// ---------------------------------------------------------------------------

/// One process as sampled; the key is `pid`.
#[derive(Debug, Clone, PartialEq)]
struct ProcessSeen {
    pid: u32,
    name: String,
}

/// One socket as sampled; the key is `(proto, local, remote)`.
#[derive(Debug, Clone, PartialEq)]
struct ConnectionSeen {
    proto: String,
    local: String,
    remote: Option<String>,
    state: String,
    pid: Option<u32>,
    process: Option<String>,
}

/// One USB device as sampled; the key is `id`.
#[derive(Debug, Clone, PartialEq)]
struct UsbSeen {
    id: String,
    name: String,
}

/// One mounted filesystem as sampled; the key is `(mount_point, device)`.
#[derive(Debug, Clone, PartialEq)]
struct MountSeen {
    mount_point: String,
    device: Option<String>,
    fs: Option<String>,
}

/// One service as sampled; the key is `name`.
#[derive(Debug, Clone, PartialEq)]
struct ServiceSeen {
    name: String,
    state: String,
}

/// Everything one sampling round saw, per family.
///
/// `None` means "not sampled or the read failed" — see [`Self::failures`] —
/// never "nothing is there": an empty family is `Some(vec![])`.
#[derive(Debug, Clone, Default)]
pub struct SystemSnapshot {
    processes: Option<Vec<ProcessSeen>>,
    connections: Option<Vec<ConnectionSeen>>,
    usb: Option<Vec<UsbSeen>>,
    mounts: Option<Vec<MountSeen>>,
    services: Option<Vec<ServiceSeen>>,
    failures: Vec<(EventType, String)>,
}

impl SystemSnapshot {
    /// Sample `types` through the managers in `context`.
    ///
    /// Never fails as a whole: a family whose read errors is recorded in
    /// [`Self::failures`] and its slot stays empty, so the round still
    /// reports everything it could see.
    pub fn sample(context: &SystemContext, types: &[EventType]) -> Self {
        let mut snapshot = Self::default();
        for event_type in EventType::ALL {
            if !types.contains(&event_type) {
                continue;
            }
            match event_type {
                EventType::Process => match sample_processes(context) {
                    Ok(rows) => snapshot.processes = Some(rows),
                    Err(error) => snapshot
                        .failures
                        .push((event_type, error.message().to_string())),
                },
                EventType::Connection => match sample_connections(context) {
                    Ok(rows) => snapshot.connections = Some(rows),
                    Err(error) => snapshot
                        .failures
                        .push((event_type, error.message().to_string())),
                },
                EventType::Usb => match sample_usb(context) {
                    Ok(rows) => snapshot.usb = Some(rows),
                    Err(error) => snapshot
                        .failures
                        .push((event_type, error.message().to_string())),
                },
                EventType::Mount => match sample_mounts(context) {
                    Ok(rows) => snapshot.mounts = Some(rows),
                    Err(error) => snapshot
                        .failures
                        .push((event_type, error.message().to_string())),
                },
                EventType::Service => match sample_services(context) {
                    Ok(rows) => snapshot.services = Some(rows),
                    Err(error) => snapshot
                        .failures
                        .push((event_type, error.message().to_string())),
                },
            }
        }
        snapshot
    }

    /// The families this round actually sampled, with their row counts.
    pub fn counts(&self) -> Vec<(EventType, usize)> {
        let mut out = Vec::new();
        if let Some(rows) = &self.processes {
            out.push((EventType::Process, rows.len()));
        }
        if let Some(rows) = &self.connections {
            out.push((EventType::Connection, rows.len()));
        }
        if let Some(rows) = &self.usb {
            out.push((EventType::Usb, rows.len()));
        }
        if let Some(rows) = &self.mounts {
            out.push((EventType::Mount, rows.len()));
        }
        if let Some(rows) = &self.services {
            out.push((EventType::Service, rows.len()));
        }
        out
    }

    /// Reads that failed this round: `(family, reason)`.
    pub fn failures(&self) -> &[(EventType, String)] {
        &self.failures
    }
}

/// Differences between two snapshots, in family order.
///
/// A family is compared only when both sides sampled it; see the module
/// docs for why.
pub fn diff(previous: &SystemSnapshot, current: &SystemSnapshot) -> Vec<SystemEvent> {
    let mut events = Vec::new();
    if let (Some(before), Some(after)) = (&previous.processes, &current.processes) {
        diff_sorted(before, after, cmp_processes, process_event, &mut events);
    }
    if let (Some(before), Some(after)) = (&previous.connections, &current.connections) {
        diff_sorted(
            before,
            after,
            cmp_connections,
            connection_event,
            &mut events,
        );
    }
    if let (Some(before), Some(after)) = (&previous.usb, &current.usb) {
        diff_sorted(before, after, cmp_usb, usb_event, &mut events);
    }
    if let (Some(before), Some(after)) = (&previous.mounts, &current.mounts) {
        diff_sorted(before, after, cmp_mounts, mount_event, &mut events);
    }
    if let (Some(before), Some(after)) = (&previous.services, &current.services) {
        diff_services(before, after, &mut events);
    }
    events
}

// ---------------------------------------------------------------------------
// Sampling: one read per family, reduced to the comparable record
// ---------------------------------------------------------------------------

fn sample_processes(context: &SystemContext) -> Result<Vec<ProcessSeen>> {
    let rows = context.process.list(&ProcessListOptions::light())?;
    let mut seen: Vec<ProcessSeen> = rows
        .into_iter()
        .map(|row| ProcessSeen {
            pid: row.pid,
            name: row.name,
        })
        .collect();
    seen.sort_by_key(|row| row.pid);
    // A duplicated pid row would read as a phantom stop; drop it instead.
    seen.dedup_by_key(|row| row.pid);
    Ok(seen)
}

fn sample_connections(context: &SystemContext) -> Result<Vec<ConnectionSeen>> {
    let rows = context.port.list(&PortListOptions::default())?;
    let mut seen: Vec<ConnectionSeen> = rows
        .iter()
        .map(|row| ConnectionSeen {
            proto: row.protocol.name().to_string(),
            local: row.path.clone().unwrap_or_else(|| row.endpoint()),
            remote: row.remote_socket_addr().map(|addr| addr.to_string()),
            state: state_word(row.state),
            pid: row.pid,
            process: row.process_name.clone(),
        })
        .collect();
    seen.sort_by(cmp_connections);
    Ok(seen)
}

fn sample_usb(context: &SystemContext) -> Result<Vec<UsbSeen>> {
    let Some(devices) = context.device.as_ref() else {
        return Err(Error::unsupported("no device capability in this context"));
    };
    let rows = devices.devices()?;
    let mut seen: Vec<UsbSeen> = rows
        .into_iter()
        .filter(|row| row.class == DeviceClass::Usb)
        .map(|row| UsbSeen {
            id: row.id.clone().unwrap_or_else(|| row.name.clone()),
            name: row.name,
        })
        .collect();
    seen.sort_by(|a, b| a.id.cmp(&b.id));
    seen.dedup_by(|a, b| a.id == b.id);
    Ok(seen)
}

fn sample_mounts(context: &SystemContext) -> Result<Vec<MountSeen>> {
    let rows = context.disk.list()?;
    let mut seen: Vec<MountSeen> = rows
        .into_iter()
        .map(|row| MountSeen {
            mount_point: row.mount_point,
            device: row.name,
            fs: row.file_system,
        })
        .collect();
    seen.sort_by(cmp_mounts);
    Ok(seen)
}

fn sample_services(context: &SystemContext) -> Result<Vec<ServiceSeen>> {
    let rows = context.service.list(&ServiceListOptions::default())?;
    let mut seen: Vec<ServiceSeen> = rows
        .into_iter()
        .map(|row| ServiceSeen {
            name: row.name,
            state: state_word(row.state),
        })
        .collect();
    seen.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(seen)
}

/// The state word the models serialize to (`running`, `time_wait`, …).
fn state_word(state: impl Serialize) -> String {
    serde_json::to_value(state)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_else(|| "unknown".to_string())
}

// ---------------------------------------------------------------------------
// Diffing
// ---------------------------------------------------------------------------

/// Two-pointer merge over two key-sorted slices.
///
/// `cmp` compares the *keys* of two records; equal keys pair up, a record on
/// one side with no partner becomes removed (before) or added (after).
fn diff_sorted<T>(
    before: &[T],
    after: &[T],
    cmp: impl Fn(&T, &T) -> std::cmp::Ordering,
    emit: impl Fn(EventOp, &T) -> SystemEvent,
    out: &mut Vec<SystemEvent>,
) {
    use std::cmp::Ordering;
    let (mut i, mut j) = (0, 0);
    loop {
        let order = match (before.get(i), after.get(j)) {
            (Some(a), Some(b)) => cmp(a, b),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => break,
        };
        match order {
            Ordering::Equal => {
                i += 1;
                j += 1;
            }
            // before[i]'s key is smaller: no partner left on the after side.
            Ordering::Less => {
                out.push(emit(EventOp::Removed, &before[i]));
                i += 1;
            }
            // after[j]'s key is smaller: no partner left on the before side.
            Ordering::Greater => {
                out.push(emit(EventOp::Added, &after[j]));
                j += 1;
            }
        }
    }
}

/// Services also compare their state when the key matches.
fn diff_services(before: &[ServiceSeen], after: &[ServiceSeen], out: &mut Vec<SystemEvent>) {
    use std::cmp::Ordering;
    let (mut i, mut j) = (0, 0);
    loop {
        match (before.get(i), after.get(j)) {
            (Some(old), Some(new)) => match old.name.cmp(&new.name) {
                Ordering::Equal => {
                    if old.state != new.state {
                        out.push(service_changed(old, new));
                    }
                    i += 1;
                    j += 1;
                }
                Ordering::Less => {
                    out.push(service_event(EventOp::Removed, old));
                    i += 1;
                }
                Ordering::Greater => {
                    out.push(service_event(EventOp::Added, new));
                    j += 1;
                }
            },
            (Some(old), None) => {
                out.push(service_event(EventOp::Removed, old));
                i += 1;
            }
            (None, Some(new)) => {
                out.push(service_event(EventOp::Added, new));
                j += 1;
            }
            (None, None) => break,
        }
    }
}

fn cmp_processes(a: &ProcessSeen, b: &ProcessSeen) -> std::cmp::Ordering {
    a.pid.cmp(&b.pid)
}

fn cmp_connections(a: &ConnectionSeen, b: &ConnectionSeen) -> std::cmp::Ordering {
    (a.proto.as_str(), a.local.as_str(), a.remote.as_deref()).cmp(&(
        b.proto.as_str(),
        b.local.as_str(),
        b.remote.as_deref(),
    ))
}

fn cmp_usb(a: &UsbSeen, b: &UsbSeen) -> std::cmp::Ordering {
    a.id.cmp(&b.id)
}

fn cmp_mounts(a: &MountSeen, b: &MountSeen) -> std::cmp::Ordering {
    (a.mount_point.as_str(), a.device.as_deref())
        .cmp(&(b.mount_point.as_str(), b.device.as_deref()))
}

// ---------------------------------------------------------------------------
// Event construction
// ---------------------------------------------------------------------------

fn process_event(op: EventOp, row: &ProcessSeen) -> SystemEvent {
    let verb = match op {
        EventOp::Removed => "stopped",
        _ => "started",
    };
    SystemEvent {
        event_type: EventType::Process,
        op,
        summary: format!("{verb} {} (pid {})", row.name, row.pid),
        pid: Some(row.pid),
        name: Some(row.name.clone()),
        detail: None,
    }
}

fn connection_event(op: EventOp, row: &ConnectionSeen) -> SystemEvent {
    let verb = match op {
        EventOp::Removed => "closed",
        _ => "opened",
    };
    let mut summary = match &row.remote {
        Some(remote) => format!(
            "{verb} {} {} -> {} [{}]",
            row.proto, row.local, remote, row.state
        ),
        None => format!("{verb} {} {} [{}]", row.proto, row.local, row.state),
    };
    if let Some(process) = &row.process {
        let pid = row
            .pid
            .map(|pid| format!(", pid {pid}"))
            .unwrap_or_default();
        summary.push_str(&format!(" ({process}{pid})"));
    }
    SystemEvent {
        event_type: EventType::Connection,
        op,
        summary,
        pid: row.pid,
        name: row.process.clone(),
        detail: None,
    }
}

fn usb_event(op: EventOp, row: &UsbSeen) -> SystemEvent {
    let verb = match op {
        EventOp::Removed => "detached",
        _ => "attached",
    };
    SystemEvent {
        event_type: EventType::Usb,
        op,
        summary: format!("{verb} {}", row.name),
        pid: None,
        name: Some(row.name.clone()),
        detail: Some(row.id.clone()),
    }
}

fn mount_event(op: EventOp, row: &MountSeen) -> SystemEvent {
    let verb = match op {
        EventOp::Removed => "unmounted",
        _ => "mounted",
    };
    let device = row
        .device
        .as_ref()
        .map(|device| format!(" ({device})"))
        .unwrap_or_default();
    SystemEvent {
        event_type: EventType::Mount,
        op,
        summary: format!("{verb} {}{device}", row.mount_point),
        pid: None,
        name: row.device.clone(),
        detail: row.fs.clone(),
    }
}

fn service_event(op: EventOp, row: &ServiceSeen) -> SystemEvent {
    let summary = match op {
        EventOp::Removed => format!("{}: disappeared", row.name),
        _ => format!("{}: appeared ({})", row.name, row.state),
    };
    SystemEvent {
        event_type: EventType::Service,
        op,
        summary,
        pid: None,
        name: Some(row.name.clone()),
        detail: Some(row.state.clone()),
    }
}

fn service_changed(before: &ServiceSeen, after: &ServiceSeen) -> SystemEvent {
    SystemEvent {
        event_type: EventType::Service,
        op: EventOp::Changed,
        summary: format!("{}: {} -> {}", before.name, before.state, after.state),
        pid: None,
        name: Some(after.name.clone()),
        detail: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{stub_context, stub_process, stub_socket, Stubs};

    fn process(pid: u32, name: &str) -> ProcessSeen {
        ProcessSeen {
            pid,
            name: name.to_string(),
        }
    }

    fn connection(proto: &str, local: &str, remote: Option<&str>, state: &str) -> ConnectionSeen {
        ConnectionSeen {
            proto: proto.to_string(),
            local: local.to_string(),
            remote: remote.map(str::to_string),
            state: state.to_string(),
            pid: None,
            process: None,
        }
    }

    fn service(name: &str, state: &str) -> ServiceSeen {
        ServiceSeen {
            name: name.to_string(),
            state: state.to_string(),
        }
    }

    fn snapshot_processes(rows: Vec<ProcessSeen>) -> SystemSnapshot {
        SystemSnapshot {
            processes: Some(rows),
            ..Default::default()
        }
    }

    #[test]
    fn process_start_and_stop_diff_into_events() {
        let before = snapshot_processes(vec![process(1, "init"), process(2, "worker")]);
        let after = snapshot_processes(vec![process(1, "init"), process(3, "builder")]);
        let events = diff(&before, &after);
        assert_eq!(events.len(), 2, "{events:?}");
        assert_eq!(events[0].event_type, EventType::Process);
        assert_eq!(events[0].op, EventOp::Removed);
        assert_eq!(events[0].summary, "stopped worker (pid 2)");
        assert_eq!(events[0].pid, Some(2));
        assert_eq!(events[1].op, EventOp::Added);
        assert_eq!(events[1].summary, "started builder (pid 3)");
        assert_eq!(events[1].name.as_deref(), Some("builder"));
    }

    #[test]
    fn identical_snapshots_diff_to_nothing() {
        let before = snapshot_processes(vec![process(1, "init")]);
        let after = snapshot_processes(vec![process(1, "init")]);
        assert!(diff(&before, &after).is_empty());
    }

    #[test]
    fn a_failing_family_never_manufactures_events() {
        let one = SystemSnapshot {
            connections: Some(vec![connection("tcp", "0.0.0.0:80", None, "listen")]),
            ..Default::default()
        };
        let blind = SystemSnapshot {
            connections: None,
            failures: vec![(EventType::Connection, "socket table unreadable".to_string())],
            ..Default::default()
        };
        let two = SystemSnapshot {
            connections: Some(vec![connection("tcp", "0.0.0.0:443", None, "listen")]),
            ..Default::default()
        };

        assert!(
            diff(&one, &blind).is_empty(),
            "a failed read must not look like every socket closed"
        );
        assert!(
            diff(&blind, &two).is_empty(),
            "recovery re-baselines instead of reporting fake opens"
        );
        assert_eq!(
            diff(&one, &two).len(),
            2,
            "two good samples compare normally"
        );
    }

    #[test]
    fn connection_events_carry_endpoints_state_and_owner() {
        let before = SystemSnapshot {
            connections: Some(vec![connection("tcp", "0.0.0.0:80", None, "listen")]),
            ..Default::default()
        };
        let mut opened = connection(
            "tcp",
            "127.0.0.1:5432",
            Some("127.0.0.1:52100"),
            "established",
        );
        opened.pid = Some(42);
        opened.process = Some("postgres".to_string());
        let after = SystemSnapshot {
            connections: Some(vec![
                connection("tcp", "0.0.0.0:80", None, "listen"),
                opened,
            ]),
            ..Default::default()
        };

        let events = diff(&before, &after);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].op, EventOp::Added);
        assert_eq!(
            events[0].summary,
            "opened tcp 127.0.0.1:5432 -> 127.0.0.1:52100 [established] (postgres, pid 42)"
        );
        assert_eq!(events[0].name.as_deref(), Some("postgres"));
        assert_eq!(events[0].pid, Some(42));
    }

    #[test]
    fn listener_lines_skip_the_arrow() {
        let before = SystemSnapshot {
            connections: Some(vec![connection("tcp", "0.0.0.0:80", None, "listen")]),
            ..Default::default()
        };
        let after = SystemSnapshot {
            connections: Some(vec![]),
            ..Default::default()
        };
        let events = diff(&before, &after);
        assert_eq!(events[0].summary, "closed tcp 0.0.0.0:80 [listen]");
    }

    #[test]
    fn service_state_changes_are_changed_events() {
        let before = SystemSnapshot {
            services: Some(vec![service("cups", "running")]),
            ..Default::default()
        };
        let after = SystemSnapshot {
            services: Some(vec![service("cups", "stopped")]),
            ..Default::default()
        };
        let events = diff(&before, &after);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].event_type, EventType::Service);
        assert_eq!(events[0].op, EventOp::Changed);
        assert_eq!(events[0].summary, "cups: running -> stopped");
        assert_eq!(events[0].name.as_deref(), Some("cups"));
    }

    #[test]
    fn services_appear_and_disappear() {
        let before = SystemSnapshot {
            services: Some(vec![service("cups", "running")]),
            ..Default::default()
        };
        let after = SystemSnapshot {
            services: Some(vec![service("cups", "running"), service("sshd", "running")]),
            ..Default::default()
        };
        let events = diff(&before, &after);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].op, EventOp::Added);
        assert_eq!(events[0].summary, "sshd: appeared (running)");
    }

    #[test]
    fn a_swapped_device_at_the_same_mount_point_is_remove_plus_add() {
        let mount = |device: &str, fs: &str| MountSeen {
            mount_point: "/media/usb".to_string(),
            device: Some(device.to_string()),
            fs: Some(fs.to_string()),
        };
        let before = SystemSnapshot {
            mounts: Some(vec![mount("sdb1", "vfat")]),
            ..Default::default()
        };
        let after = SystemSnapshot {
            mounts: Some(vec![mount("sdc1", "exfat")]),
            ..Default::default()
        };
        let events = diff(&before, &after);
        assert_eq!(events.len(), 2, "{events:?}");
        assert_eq!(events[0].summary, "unmounted /media/usb (sdb1)");
        assert_eq!(events[1].summary, "mounted /media/usb (sdc1)");
        assert_eq!(events[1].event_type, EventType::Mount);
        assert_eq!(events[1].detail.as_deref(), Some("exfat"));
    }

    #[test]
    fn usb_events_key_on_the_platform_id() {
        let before = SystemSnapshot {
            usb: Some(vec![UsbSeen {
                id: "USB\\VID_1".to_string(),
                name: "Stick".to_string(),
            }]),
            ..Default::default()
        };
        let after = SystemSnapshot {
            usb: Some(vec![]),
            ..Default::default()
        };
        let events = diff(&before, &after);
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].summary, "detached Stick");
        assert_eq!(events[0].event_type, EventType::Usb);
        assert_eq!(events[0].detail.as_deref(), Some("USB\\VID_1"));
    }

    #[test]
    fn sample_honours_the_filter_and_reports_unreadable_families() {
        let stubs = Stubs::new()
            .with_processes(vec![stub_process(7, None, "solo")])
            .with_ports(vec![stub_socket(80, 7, "solo")])
            .with_devices(vec![]);
        let context = stubs.context();

        let snapshot = SystemSnapshot::sample(&context, &[EventType::Process, EventType::Usb]);
        assert_eq!(
            snapshot.counts(),
            vec![(EventType::Process, 1), (EventType::Usb, 0)]
        );
        assert!(snapshot.failures().is_empty(), "{:?}", snapshot.failures());

        // `stub_context` carries no optional capabilities: usb has no reader.
        let bare = SystemSnapshot::sample(&stub_context(), &[EventType::Usb]);
        assert!(bare.counts().is_empty());
        assert_eq!(bare.failures().len(), 1);
        assert_eq!(bare.failures()[0].0, EventType::Usb);
        assert!(
            bare.failures()[0].1.contains("no device capability"),
            "{:?}",
            bare.failures()
        );
    }

    #[test]
    fn events_serialize_only_what_is_known() {
        let event = SystemEvent {
            event_type: EventType::Mount,
            op: EventOp::Added,
            summary: "mounted D: (D:)".to_string(),
            pid: None,
            name: Some("D:".to_string()),
            detail: Some("ntfs".to_string()),
        };
        let value = serde_json::to_value(&event).expect("json");
        assert_eq!(value["type"], "mount");
        assert_eq!(value["op"], "added");
        assert_eq!(value["summary"], "mounted D: (D:)");
        assert!(value.get("pid").is_none(), "unknown fields must be absent");
        assert_eq!(value["detail"], "ntfs");
    }

    #[test]
    fn types_parse_from_their_canonical_tokens() {
        assert_eq!(EventType::parse("usb"), Some(EventType::Usb));
        assert_eq!(EventType::parse("Process"), Some(EventType::Process));
        assert_eq!(EventType::parse("everything"), None);
        assert!(!EventType::DEFAULT.contains(&EventType::Usb));
    }
}
