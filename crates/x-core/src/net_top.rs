//! Per-process network usage, computed by comparing two snapshots.
//!
//! The model is the same snapshot-diff pairing `events` uses: a sampler
//! (per-platform, see `x-platform`) produces a [`NetSnapshot`] holding the
//! per-interface byte counters, the connection table with process ownership,
//! and — where the platform can provide it — per-PID byte counters for the
//! interval. The pure functions here turn two snapshots into per-process
//! rates. Everything that can fail is `Option`-shaped: "could not read" is
//! never rendered as "zero" or "gone".

use std::collections::BTreeMap;
use std::time::Duration;

/// Cumulative interface byte counters at one sampling instant.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InterfaceCounters {
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// One sampled connection with its owning process, if attribution succeeded.
///
/// This mirrors the data the port manager already reads; the struct is local
/// to this module so the diff logic stays independent of `port`'s richer row
/// shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionOwner {
    /// "1.2.3.4:443" style endpoint of the remote side, local sockets "-".
    pub remote: String,
    /// Owning pid; `None` when the platform cannot attribute this socket.
    pub pid: Option<i32>,
}

/// Everything one sampling round saw about the network.
#[derive(Debug, Clone, Default)]
pub struct NetSnapshot {
    /// ifIndex/name -> cumulative counters. `None` = the read failed.
    pub interfaces: Option<BTreeMap<String, InterfaceCounters>>,
    /// Connections with owners. `None` = the read failed.
    pub connections: Option<Vec<ConnectionOwner>>,
    /// Per-pid bytes transferred during the interval ending at this snapshot.
    ///
    /// Only a platform with a first-class per-process source (Windows ETW)
    /// fills this. `None` = the source is absent or the read failed — never
    /// an empty map meaning "nobody transferred anything".
    pub pid_bytes: Option<BTreeMap<i32, PidBytes>>,
    /// Reads that failed this round: (layer, reason). A failed layer does not
    /// participate in the diff; the next successful round becomes the baseline.
    pub failures: Vec<(String, String)>,
}

/// Byte counters one process accumulated over one interval.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PidBytes {
    pub rx: u64,
    pub tx: u64,
}

/// Per-process rate over the interval between two snapshots.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcessRate {
    pub pid: i32,
    /// Bytes per second, only when a first-class per-process source provided
    /// counters for both endpoints of the interval.
    pub rx_bps: Option<f64>,
    pub tx_bps: Option<f64>,
    /// Connections owned at the current instant.
    pub conns: usize,
    /// How rx/tx were obtained: "etw" (kernel-reported) or "port-inference"
    /// (packet capture attributed through the connection table).
    pub source: &'static str,
}

/// Rate for traffic the connection table could not attribute to a process.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct UnmappedRate {
    pub rx_bps: Option<f64>,
    pub tx_bps: Option<f64>,
}

/// One round of `x net top`: host totals plus the per-process breakdown.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NetTopReport {
    pub interval: Duration,
    pub host_rx_bps: Option<f64>,
    pub host_tx_bps: Option<f64>,
    pub processes: Vec<ProcessRate>,
    /// Host-level traffic that no process row claims.
    pub unmapped: UnmappedRate,
    /// Layers that failed this round, verbatim from the snapshot.
    pub failures: Vec<(String, String)>,
}

/// Rate from two cumulative counter readings, or `None` when the pair is
/// unusable: a missing endpoint (read failed) or a counter that went
/// backwards (interface reset — drop the round rather than report a lie).
pub fn counter_rate(
    previous: Option<&InterfaceCounters>,
    current: Option<&InterfaceCounters>,
    interval: Duration,
) -> Option<(f64, f64)> {
    let (prev, curr) = (previous?, current?);
    if interval.is_zero() {
        return None;
    }
    if curr.rx_bytes < prev.rx_bytes || curr.tx_bytes < prev.tx_bytes {
        return None;
    }
    let secs = interval.as_secs_f64();
    Some((
        (curr.rx_bytes - prev.rx_bytes) as f64 / secs,
        (curr.tx_bytes - prev.tx_bytes) as f64 / secs,
    ))
}

/// Aggregate the connection table by owning pid.
fn conns_by_pid(connections: &[ConnectionOwner]) -> BTreeMap<i32, usize> {
    let mut counts: BTreeMap<i32, usize> = BTreeMap::new();
    for conn in connections {
        if let Some(pid) = conn.pid {
            *counts.entry(pid).or_default() += 1;
        }
    }
    counts
}

/// Turn two snapshots into one report. Either side may lack a layer; the
/// functions below handle each independently and [`NetTopReport`] carries the
/// union of what both rounds saw.
pub fn diff(previous: &NetSnapshot, current: &NetSnapshot, interval: Duration) -> NetTopReport {
    let mut report = NetTopReport {
        interval,
        failures: current.failures.clone(),
        ..NetTopReport::default()
    };

    // L1: host rate from interface counters. A backwards counter (interface
    // reset) yields None for this round — report the absence, not a negative.
    let prev_ifs = previous.interfaces.as_ref();
    let curr_ifs = current.interfaces.as_ref();
    if curr_ifs.is_none() {
        report
            .failures
            .push(("interfaces".into(), "read failed this round".into()));
    }
    if let (Some(prev), Some(curr)) = (prev_ifs, curr_ifs) {
        let mut rx = 0.0f64;
        let mut tx = 0.0f64;
        let mut any = false;
        for (name, curr_counters) in curr {
            // An interface that vanished since last round has no baseline;
            // skip it instead of treating its counters as fresh.
            let Some(prev_counters) = prev.get(name) else {
                continue;
            };
            if let Some((r, t)) = counter_rate(Some(prev_counters), Some(curr_counters), interval) {
                rx += r;
                tx += t;
                any = true;
            }
        }
        if any {
            report.host_rx_bps = Some(rx);
            report.host_tx_bps = Some(tx);
        }
    }

    // L2: connection ownership from the current instant (no diffing — a
    // connection list is a fact about now, not an interval).
    let conns: &Vec<ConnectionOwner> = match &current.connections {
        Some(c) => c,
        None => {
            report
                .failures
                .push(("connections".into(), "read failed this round".into()));
            &Vec::new()
        }
    };
    let per_pid_conns = conns_by_pid(conns);

    // Traffic the connection table could not attribute: host total minus the
    // sum of per-process rates, computed per layer only when both exist.
    // Computed before the early return below — a platform without a
    // per-process source still has real unmapped traffic.
    let (host_rx, host_tx) = (report.host_rx_bps, report.host_tx_bps);

    // L3: per-process rates. Only present when the sampler provided counters
    // for both rounds; "port-inference" sources produce them the same way,
    // tagged differently.
    let (prev_pids, curr_pids) = match (&previous.pid_bytes, &current.pid_bytes) {
        (Some(p), Some(c)) => (p, c),
        _ => {
            // No per-process layer on this platform or it failed. Attribute
            // nothing; conns still render below.
            report.unmapped = UnmappedRate {
                rx_bps: host_rx,
                tx_bps: host_tx,
            };
            report.processes = per_pid_conns
                .into_iter()
                .map(|(pid, conns)| ProcessRate {
                    pid,
                    rx_bps: None,
                    tx_bps: None,
                    conns,
                    source: "connections-only",
                })
                .collect();
            return report;
        }
    };

    let secs = interval.as_secs_f64();
    report.processes = curr_pids
        .iter()
        .map(|(pid, curr_bytes)| {
            let (rx, tx) = match prev_pids.get(pid) {
                // A backwards counter is as unusable as a missing one.
                Some(prev_bytes)
                    if curr_bytes.rx >= prev_bytes.rx && curr_bytes.tx >= prev_bytes.tx =>
                {
                    (
                        Some((curr_bytes.rx - prev_bytes.rx) as f64 / secs),
                        Some((curr_bytes.tx - prev_bytes.tx) as f64 / secs),
                    )
                }
                _ => (None, None),
            };
            let source = current_source_tag(curr_pids);
            ProcessRate {
                pid: *pid,
                rx_bps: rx,
                tx_bps: tx,
                conns: per_pid_conns.get(pid).copied().unwrap_or(0),
                source,
            }
        })
        .collect();

    // Traffic the connection table could not attribute: host total minus the
    // sum of per-process rates, computed per layer only when both exist.
    if let (Some(host_rx), Some(host_tx)) = (report.host_rx_bps, report.host_tx_bps) {
        let attributed_rx: f64 = report.processes.iter().filter_map(|p| p.rx_bps).sum();
        let attributed_tx: f64 = report.processes.iter().filter_map(|p| p.tx_bps).sum();
        report.unmapped = UnmappedRate {
            rx_bps: Some((host_rx - attributed_rx).max(0.0)),
            tx_bps: Some((host_tx - attributed_tx).max(0.0)),
        };
    }

    report
}

/// The platform tags every snapshot's counters with one source; the first
/// non-empty tag wins. Kept per-snapshot rather than per-process so the
/// renderer can state it once.
fn current_source_tag(counters: &BTreeMap<i32, PidBytes>) -> &'static str {
    // The sampler decides the tag by how it filled the map; the model can
    // only record what it was told. Read from the first entry's metadata
    // lane — encoded by the sampler as a negative sentinel pid.
    match counters.get(&SOURCE_SENTINEL) {
        Some(_) => "port-inference",
        None => "etw",
    }
}

/// Reserved pid the sampler uses to tag how the per-process counters were
/// obtained. A real kernel never hands out this pid.
pub const SOURCE_SENTINEL: i32 = i32::MAX;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn ifmap(entries: &[(&str, u64, u64)]) -> Option<BTreeMap<String, InterfaceCounters>> {
        Some(
            entries
                .iter()
                .map(|(n, r, t)| {
                    (
                        n.to_string(),
                        InterfaceCounters {
                            rx_bytes: *r,
                            tx_bytes: *t,
                        },
                    )
                })
                .collect(),
        )
    }

    fn conns(entries: &[(i32, &str)]) -> Option<Vec<ConnectionOwner>> {
        Some(
            entries
                .iter()
                .map(|(pid, remote)| ConnectionOwner {
                    pid: Some(*pid),
                    remote: remote.to_string(),
                })
                .collect(),
        )
    }

    const TWO_SECONDS: Duration = Duration::from_secs(2);

    #[test]
    fn host_rate_is_counter_delta_over_interval() {
        let prev = NetSnapshot {
            interfaces: ifmap(&[("en0", 1000, 2000)]),
            ..Default::default()
        };
        let curr = NetSnapshot {
            interfaces: ifmap(&[("en0", 3000, 2000)]),
            ..Default::default()
        };
        let report = diff(&prev, &curr, TWO_SECONDS);
        assert_eq!(report.host_rx_bps, Some(1000.0));
        assert_eq!(report.host_tx_bps, Some(0.0));
    }

    #[test]
    fn interfaces_added_since_baseline_are_skipped() {
        let prev = NetSnapshot {
            interfaces: ifmap(&[("en0", 0, 0)]),
            ..Default::default()
        };
        let curr = NetSnapshot {
            interfaces: ifmap(&[("en0", 100, 100), ("utun4", 999_999, 999_999)]),
            ..Default::default()
        };
        let report = diff(&prev, &curr, TWO_SECONDS);
        // utun4 just appeared: counting it from zero would report its whole
        // life as this interval's traffic.
        assert_eq!(report.host_rx_bps, Some(50.0));
    }

    #[test]
    fn backwards_counter_drops_the_round() {
        let prev = NetSnapshot {
            interfaces: ifmap(&[("en0", 5000, 5000)]),
            ..Default::default()
        };
        let curr = NetSnapshot {
            interfaces: ifmap(&[("en0", 10, 10)]),
            ..Default::default()
        };
        let report = diff(&prev, &curr, TWO_SECONDS);
        assert_eq!(report.host_rx_bps, None);
        assert_eq!(report.host_tx_bps, None);
    }

    #[test]
    fn failed_layer_is_reported_not_rendered_as_zero() {
        let prev = NetSnapshot::default();
        let curr = NetSnapshot {
            interfaces: None,
            failures: vec![("interfaces".into(), "permission denied".into())],
            ..Default::default()
        };
        let report = diff(&prev, &curr, TWO_SECONDS);
        assert_eq!(report.host_rx_bps, None);
        assert!(report
            .failures
            .iter()
            .any(|(layer, reason)| layer == "interfaces" && reason.contains("permission")));
    }

    #[test]
    fn per_process_rates_only_when_both_rounds_have_counters() {
        let pid_bytes = |rx: u64, tx: u64| PidBytes { rx, tx };
        let mut prev_pids = BTreeMap::new();
        prev_pids.insert(42, pid_bytes(1000, 100));
        let mut curr_pids = BTreeMap::new();
        curr_pids.insert(42, pid_bytes(3000, 100));
        let prev = NetSnapshot {
            pid_bytes: Some(prev_pids),
            ..Default::default()
        };
        let curr = NetSnapshot {
            pid_bytes: Some(curr_pids),
            connections: conns(&[(42, "1.2.3.4:443")]),
            ..Default::default()
        };
        let report = diff(&prev, &curr, TWO_SECONDS);
        assert_eq!(report.processes.len(), 1);
        let proc = &report.processes[0];
        assert_eq!(proc.rx_bps, Some(1000.0));
        assert_eq!(proc.tx_bps, Some(0.0));
        assert_eq!(proc.conns, 1);
    }

    #[test]
    fn new_pid_has_no_rate_until_a_baseline_exists() {
        let mut curr_pids = BTreeMap::new();
        curr_pids.insert(7, PidBytes { rx: 500, tx: 500 });
        let prev = NetSnapshot {
            pid_bytes: Some(BTreeMap::new()),
            ..Default::default()
        };
        let curr = NetSnapshot {
            pid_bytes: Some(curr_pids),
            ..Default::default()
        };
        let report = diff(&prev, &curr, TWO_SECONDS);
        assert_eq!(report.processes[0].rx_bps, None);
        assert_eq!(report.processes[0].tx_bps, None);
    }

    #[test]
    fn platform_without_per_process_source_still_reports_conns() {
        let prev = NetSnapshot::default();
        let curr = NetSnapshot {
            connections: conns(&[(42, "1.2.3.4:443"), (42, "5.6.7.8:80"), (9, "1.1.1.1:53")]),
            ..Default::default()
        };
        let report = diff(&prev, &curr, TWO_SECONDS);
        let p42 = report.processes.iter().find(|p| p.pid == 42).unwrap();
        assert_eq!(p42.conns, 2);
        assert_eq!(p42.rx_bps, None);
        assert_eq!(p42.source, "connections-only");
    }

    #[test]
    fn unmapped_is_host_total_minus_attributed() {
        let prev = NetSnapshot {
            interfaces: ifmap(&[("en0", 0, 0)]),
            // A baseline for pid 42, so its rate over the interval is known.
            pid_bytes: Some({
                let mut m = BTreeMap::new();
                m.insert(42, PidBytes { rx: 0, tx: 0 });
                m
            }),
            ..Default::default()
        };
        let curr = NetSnapshot {
            interfaces: ifmap(&[("en0", 2000, 1000)]),
            pid_bytes: Some({
                let mut m = BTreeMap::new();
                m.insert(42, PidBytes { rx: 1000, tx: 400 });
                m
            }),
            ..Default::default()
        };
        let report = diff(&prev, &curr, TWO_SECONDS);
        let unmapped = &report.unmapped;
        assert_eq!(unmapped.rx_bps, Some(500.0));
        assert_eq!(unmapped.tx_bps, Some(300.0));
    }

    #[test]
    fn no_per_process_baseline_leaves_everything_unmapped() {
        // One round without counters: the pid rates are unknowable, so the
        // honest attribution is "all of it unmapped", not a guess.
        let prev = NetSnapshot {
            interfaces: ifmap(&[("en0", 0, 0)]),
            ..Default::default()
        };
        let curr = NetSnapshot {
            interfaces: ifmap(&[("en0", 2000, 1000)]),
            pid_bytes: Some({
                let mut m = BTreeMap::new();
                m.insert(42, PidBytes { rx: 1000, tx: 400 });
                m
            }),
            ..Default::default()
        };
        let report = diff(&prev, &curr, TWO_SECONDS);
        assert_eq!(report.unmapped.rx_bps, Some(1000.0));
        assert_eq!(report.unmapped.tx_bps, Some(500.0));
        assert!(report.processes.iter().all(|p| p.rx_bps.is_none()));
    }

    #[test]
    fn sampler_sentinel_tags_the_source() {
        let mut curr_pids = BTreeMap::new();
        curr_pids.insert(SOURCE_SENTINEL, PidBytes::default());
        curr_pids.insert(42, PidBytes { rx: 1, tx: 1 });
        let prev = NetSnapshot {
            pid_bytes: Some(curr_pids.clone()),
            ..Default::default()
        };
        let curr = NetSnapshot {
            pid_bytes: Some(curr_pids),
            ..Default::default()
        };
        let report = diff(&prev, &curr, TWO_SECONDS);
        assert_eq!(report.processes[0].source, "port-inference");
    }

    #[test]
    fn zero_interval_never_divides() {
        let counters = InterfaceCounters {
            rx_bytes: 100,
            tx_bytes: 100,
        };
        assert_eq!(
            counter_rate(Some(&counters), Some(&counters), Duration::ZERO),
            None
        );
    }
}
