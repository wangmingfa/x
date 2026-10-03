//! Performance baseline: how expensive each listing read is on this machine.
//!
//! [`crate::capability`] answers whether a read works here; this module answers
//! what it costs. The purpose is regression detection: an adapter that starts
//! doing one syscall per row, or a grouping that quietly becomes quadratic,
//! still looks fine on a laptop with 300 processes and only reveals itself on a
//! server with 3000. Timing the real adapters and dividing by the row count the
//! adapter actually returned keeps the two comparable.
//!
//! The clock lives in [`bench`]. Everything derived from the samples — best,
//! median, per-row cost, budget status, scale notes — is decided by pure
//! functions over milliseconds, so what a number *means* is testable on a fast
//! machine that produces no interesting timings.

use serde::{Deserialize, Serialize};
use std::time::Instant;

use crate::context::SystemContext;
use crate::error::Result;
use crate::port::PortListOptions;
use crate::process::ProcessListOptions;

/// One timed read operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchTarget {
    /// Process table without CPU/memory accounting: the raw read.
    ProcessListLight,
    /// Process table with usage accounting, which includes the CPU sample window.
    ProcessListUsage,
    /// Process table plus parent linking.
    ProcessTree,
    /// Every socket the platform can attribute to a process.
    SocketList,
    /// Listening sockets only, which is what `x port` reads by default.
    SocketListening,
    /// Every socket, grouped per owning process.
    SocketOwners,
    /// Service units from systemd / OpenRC / SCM / launchd.
    ServiceList,
    /// Network interfaces.
    Interfaces,
    /// Addresses per interface.
    Addresses,
    /// Routing table.
    Routes,
    /// Mounted filesystems.
    Disks,
}

impl BenchTarget {
    /// Every target, in reporting order.
    pub const ALL: &'static [Self] = &[
        Self::ProcessListLight,
        Self::ProcessListUsage,
        Self::ProcessTree,
        Self::SocketList,
        Self::SocketListening,
        Self::SocketOwners,
        Self::ServiceList,
        Self::Interfaces,
        Self::Addresses,
        Self::Routes,
        Self::Disks,
    ];

    /// Human-readable name, used as the table's first column.
    pub const fn label(self) -> &'static str {
        match self {
            Self::ProcessListLight => "process list (light)",
            Self::ProcessListUsage => "process list (with usage)",
            Self::ProcessTree => "process tree",
            Self::SocketList => "socket list (all)",
            Self::SocketListening => "socket list (listening)",
            Self::SocketOwners => "socket owners",
            Self::ServiceList => "service list",
            Self::Interfaces => "interfaces",
            Self::Addresses => "addresses",
            Self::Routes => "routes",
            Self::Disks => "disk list",
        }
    }

    /// Capability domain this read belongs to, so `--domain` works the same way
    /// it does for `x capability`.
    pub const fn domain(self) -> &'static str {
        match self {
            Self::ProcessListLight | Self::ProcessListUsage | Self::ProcessTree => "process",
            Self::SocketList | Self::SocketListening | Self::SocketOwners => "port",
            Self::ServiceList => "service",
            Self::Interfaces | Self::Addresses | Self::Routes => "net",
            Self::Disks => "disk",
        }
    }

    /// Ceiling for the median sample, in milliseconds.
    ///
    /// These are deliberately generous. They are not a performance target and
    /// they do not score the machine: they are the point where an adapter that
    /// used to be fast has become an order of magnitude slower, which is the
    /// failure this command exists to catch. `--budget-ms` overrides all of
    /// them for a machine or a CI job with different expectations.
    pub const fn budget_ms(self) -> f64 {
        match self {
            Self::ProcessListLight => 400.0,
            Self::ProcessListUsage => 1_500.0,
            Self::ProcessTree => 800.0,
            Self::SocketList => 1_000.0,
            Self::SocketListening => 800.0,
            Self::SocketOwners => 1_200.0,
            Self::ServiceList => 3_000.0,
            Self::Interfaces => 300.0,
            Self::Addresses => 300.0,
            Self::Routes => 400.0,
            Self::Disks => 400.0,
        }
    }

    /// Row count the budget was calibrated for, `None` when the read's cost is
    /// dominated by fixed overhead.
    ///
    /// This is the "1000 processes / 10k sockets" scale from the roadmap. Below
    /// it a host cannot be judged on absolute milliseconds, so the report says
    /// so instead of pretending the numbers are comparable across machines. A
    /// read that pays a fixed price (four table calls, one mount table) has no
    /// row scale to compare against, so it gets no note. A filtered read is
    /// timed over the whole table but reports the surviving rows, so its row
    /// count is not the scale it was measured at either.
    pub const fn baseline_rows(self) -> Option<usize> {
        match self {
            Self::ProcessListLight | Self::ProcessListUsage | Self::ProcessTree => Some(1_000),
            Self::SocketList | Self::SocketOwners => Some(10_000),
            Self::SocketListening
            | Self::ServiceList
            | Self::Interfaces
            | Self::Addresses
            | Self::Routes
            | Self::Disks => None,
        }
    }

    /// What the measurement includes besides the adapter read.
    pub const fn caveat(self) -> Option<&'static str> {
        match self {
            Self::ProcessListUsage => {
                Some("includes the cpu sample window the platform waits for (at least 120 ms)")
            }
            Self::ProcessListLight => Some("no cpu/memory accounting: the raw kernel read"),
            _ => None,
        }
    }
}

/// How a measured target compares to its budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BenchStatus {
    /// Median stayed under the budget.
    Ok,
    /// Median exceeded the budget.
    Slow,
    /// The adapter refused the read, so there is no baseline to compare.
    Failed,
}

impl BenchStatus {
    /// Lowercase identifier, matching the JSON spelling.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Slow => "slow",
            Self::Failed => "failed",
        }
    }
}

/// One target's result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchRow {
    /// Which read this is.
    pub target: BenchTarget,
    /// Grouping key, matching `x capability`.
    pub domain: String,
    /// Rows the adapter returned at this scale.
    pub rows: usize,
    /// Timed passes behind these numbers.
    pub samples: usize,
    /// Fastest timed pass, in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub best_ms: Option<f64>,
    /// Middle of the timed passes, in milliseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub median_ms: Option<f64>,
    /// Best pass divided by the rows returned, in microseconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub us_per_row: Option<f64>,
    /// Budget this row was judged against.
    pub budget_ms: f64,
    /// Verdict.
    pub status: BenchStatus,
    /// Why it failed, or what the timing includes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A whole run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BenchReport {
    /// Timed passes per target.
    pub samples: usize,
    /// One row per measured target.
    pub rows: Vec<BenchRow>,
    /// Targets sampled below the scale their budget assumes.
    pub scale_notes: Vec<String>,
}

impl BenchReport {
    /// Targets that went over budget, in reporting order.
    pub fn slow_rows(&self) -> Vec<&BenchRow> {
        self.rows
            .iter()
            .filter(|row| row.status == BenchStatus::Slow)
            .collect()
    }
}

/// What to sample and how.
#[derive(Debug, Clone)]
pub struct BenchConfig {
    /// Timed passes per target, after one untimed warm-up pass.
    pub samples: usize,
    /// Replace every target's budget with this.
    pub budget_ms: Option<f64>,
    /// Only measure targets in this domain.
    pub domain: Option<String>,
}

impl Default for BenchConfig {
    fn default() -> Self {
        Self {
            samples: 3,
            budget_ms: None,
            domain: None,
        }
    }
}

/// Time every selected target against `context`.
pub fn bench(context: &SystemContext, config: &BenchConfig) -> BenchReport {
    let needle = config
        .domain
        .as_ref()
        .map(|domain| domain.to_ascii_lowercase());
    let mut rows = Vec::new();

    for target in BenchTarget::ALL {
        if let Some(needle) = &needle {
            if *needle != target.domain() {
                continue;
            }
        }

        // The first pass is untimed: caches are cold (the process table learns
        // names on its first refresh) and a service manager may have to start.
        // Measuring that would report the warm-up, not the adapter.
        let mut samples_ms = Vec::with_capacity(config.samples);
        let mut count = 0usize;
        let mut failure: Option<String> = None;

        for pass in 0..=config.samples {
            let started = Instant::now();
            let outcome = measure(context, *target);
            let elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
            match outcome {
                Ok(found) => {
                    count = found;
                    failure = None;
                }
                Err(error) => {
                    count = 0;
                    failure = Some(error.message().to_string());
                }
            }
            if pass > 0 {
                samples_ms.push(elapsed_ms);
            }
        }

        rows.push(row_for(
            *target,
            count,
            &samples_ms,
            config.budget_ms,
            failure.as_deref(),
        ));
    }

    let scale_notes = scale_notes(&rows);
    BenchReport {
        samples: config.samples,
        rows,
        scale_notes,
    }
}

/// Run one read and report how many rows it produced.
fn measure(context: &SystemContext, target: BenchTarget) -> Result<usize> {
    match target {
        BenchTarget::ProcessListLight => context
            .process
            .list(&ProcessListOptions::light())
            .map(|rows| rows.len()),
        BenchTarget::ProcessListUsage => context
            .process
            .list(&ProcessListOptions {
                with_usage: true,
                ..Default::default()
            })
            .map(|rows| rows.len()),
        BenchTarget::ProcessTree => context
            .process
            .tree(&ProcessListOptions::light())
            .map(|tree| tree.len()),
        BenchTarget::SocketList => context
            .port
            .list(&Default::default())
            .map(|rows| rows.len()),
        BenchTarget::SocketListening => context
            .port
            .list(&PortListOptions::listening())
            .map(|rows| rows.len()),
        BenchTarget::SocketOwners => context
            .port
            .owners(&Default::default())
            .map(|owners| owners.iter().map(|owner| owner.ports.len()).sum()),
        BenchTarget::ServiceList => context
            .service
            .list(&Default::default())
            .map(|rows| rows.len()),
        BenchTarget::Interfaces => context.network.interfaces().map(|rows| rows.len()),
        BenchTarget::Addresses => context.network.addresses().map(|rows| rows.len()),
        BenchTarget::Routes => context.network.routes().map(|rows| rows.len()),
        BenchTarget::Disks => context.disk.list().map(|rows| rows.len()),
    }
}

/// Build a row from milliseconds already measured elsewhere.
///
/// The budget comparison uses the unrounded median while the reported numbers
/// are rounded for display, so a target is never called fast because its
/// sub-millisecond duration printed as `0.00`.
pub fn row_for(
    target: BenchTarget,
    rows: usize,
    samples_ms: &[f64],
    budget_override: Option<f64>,
    failure: Option<&str>,
) -> BenchRow {
    let budget_ms = budget_override.unwrap_or_else(|| target.budget_ms());
    let median = median(samples_ms);
    let best = best(samples_ms);

    let status = match &failure {
        Some(_) => BenchStatus::Failed,
        None => match median {
            Some(median) if median > budget_ms => BenchStatus::Slow,
            _ => BenchStatus::Ok,
        },
    };

    let note = match (failure, target.caveat()) {
        (Some(message), _) => Some(message.to_string()),
        (None, caveat) => caveat.map(str::to_string),
    };

    BenchRow {
        target,
        domain: target.domain().to_string(),
        rows,
        samples: samples_ms.len(),
        best_ms: best.map(round2),
        median_ms: median.map(round2),
        us_per_row: per_row(best, rows).map(round1),
        budget_ms: round2(budget_ms),
        status,
        note,
    }
}

/// Fastest timed pass.
pub fn best(samples_ms: &[f64]) -> Option<f64> {
    samples_ms
        .iter()
        .copied()
        .fold(None, |acc: Option<f64>, value| {
            Some(match acc {
                Some(current) => current.min(value),
                None => value,
            })
        })
}

/// Middle of the timed passes: the average of the two central samples when the
/// count is even.
pub fn median(samples_ms: &[f64]) -> Option<f64> {
    if samples_ms.is_empty() {
        return None;
    }
    let mut sorted = samples_ms.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let middle = sorted.len() / 2;
    Some(if sorted.len() % 2 == 1 {
        sorted[middle]
    } else {
        (sorted[middle - 1] + sorted[middle]) / 2.0
    })
}

/// Microseconds per row from the best pass.
///
/// `None` when the read returned nothing: a per-row cost with no rows would
/// divide by zero, and `0.00` would read as "fast" rather than "no data".
pub fn per_row(best_ms: Option<f64>, rows: usize) -> Option<f64> {
    match best_ms {
        Some(ms) if rows > 0 => Some(ms * 1_000.0 / rows as f64),
        _ => None,
    }
}

/// Say which row-proportional targets were sampled below their baseline scale.
pub fn scale_notes(rows: &[BenchRow]) -> Vec<String> {
    rows.iter()
        .filter_map(|row| {
            let baseline = row.target.baseline_rows()?;
            if row.status == BenchStatus::Failed || row.rows >= baseline {
                return None;
            }
            Some(format!(
                "{}: {} row(s) here, the budget assumes {baseline} — these are this host's \
                 numbers, compare them with your own previous runs, not another machine's",
                row.target.label(),
                row.rows
            ))
        })
        .collect()
}

fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{stub_process, stub_service, stub_socket, StubFailure, Stubs};

    fn populated() -> SystemContext {
        Stubs::new()
            .with_ports(vec![
                stub_socket(8080, 42, "node"),
                stub_socket(53, 1, "systemd"),
            ])
            .with_processes(vec![stub_process(42, Some(1), "node")])
            .with_services(vec![stub_service("sshd", 42)])
            .context()
    }

    #[test]
    fn statistics_take_the_fastest_and_the_middle_sample() {
        let samples = [30.0, 12.5, 20.0];
        assert_eq!(best(&samples), Some(12.5));
        assert_eq!(median(&samples), Some(20.0));
        // An even count averages the two central samples.
        assert_eq!(median(&[10.0, 20.0, 30.0, 40.0]), Some(25.0));
        assert_eq!(best(&[]), None);
        assert_eq!(median(&[]), None);
    }

    #[test]
    fn per_row_cost_is_absent_without_rows() {
        assert_eq!(per_row(Some(100.0), 1_000), Some(100.0));
        assert_eq!(per_row(Some(100.0), 0), None);
        assert_eq!(per_row(None, 10), None);
    }

    #[test]
    fn a_median_over_budget_is_slow_and_under_is_ok() {
        let ok = row_for(BenchTarget::Interfaces, 4, &[10.0, 12.0, 11.0], None, None);
        assert_eq!(ok.status, BenchStatus::Ok);
        assert_eq!(ok.budget_ms, 300.0);

        let slow = row_for(
            BenchTarget::Interfaces,
            4,
            &[900.0, 1_200.0, 1_100.0],
            None,
            None,
        );
        assert_eq!(slow.status, BenchStatus::Slow);

        // The override replaces every target's own ceiling.
        let strict = row_for(BenchTarget::Interfaces, 4, &[5.0], Some(1.0), None);
        assert_eq!(strict.status, BenchStatus::Slow);
        assert_eq!(strict.budget_ms, 1.0);
    }

    #[test]
    fn a_sub_millisecond_median_is_not_rounded_into_being_fast() {
        // 0.0004 ms prints as `0.00`; against a zero budget it is still slow.
        let row = row_for(BenchTarget::Disks, 1, &[0.0004], Some(0.0), None);
        assert_eq!(row.median_ms, Some(0.0));
        assert_eq!(row.status, BenchStatus::Slow);
    }

    #[test]
    fn a_failed_adapter_is_reported_not_blanked() {
        let row = row_for(
            BenchTarget::SocketList,
            0,
            &[5.0],
            None,
            Some("this platform cannot attribute sockets to processes yet"),
        );
        assert_eq!(row.status, BenchStatus::Failed);
        assert_eq!(
            row.note.as_deref(),
            Some("this platform cannot attribute sockets to processes yet")
        );
        assert_eq!(row.rows, 0);
        // The timing is kept: the failing path still costs what it costs.
        assert_eq!(row.best_ms, Some(5.0));
    }

    #[test]
    fn a_small_host_is_told_its_absolute_numbers_are_not_the_baseline() {
        let rows = vec![
            row_for(BenchTarget::SocketList, 12, &[5.0], None, None),
            row_for(BenchTarget::SocketListening, 4, &[5.0], None, None),
            row_for(BenchTarget::Interfaces, 3, &[1.0], None, None),
            row_for(BenchTarget::ProcessListLight, 1_200, &[80.0], None, None),
        ];
        let notes = scale_notes(&rows);
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].starts_with("socket list (all): 12 row(s)"));
        assert!(notes[0].contains("the budget assumes 10000"));
        assert!(notes[0].contains("your own previous runs"));
        // A filtered read is timed over the whole table, so its surviving row
        // count is not a scale claim.
        assert!(!notes.iter().any(|note| note.contains("listening")));
        // A fixed-cost read gets no scale note at all.
        assert!(!notes.iter().any(|note| note.contains("interfaces")));
        // 1200 processes is at or above the 1000-row scale, so no note.
        assert!(!notes.iter().any(|note| note.contains("process list")));
    }

    #[test]
    fn a_failed_target_gets_no_scale_note() {
        let rows = vec![row_for(
            BenchTarget::SocketList,
            0,
            &[1.0],
            None,
            Some("no socket table on this host"),
        )];
        assert!(scale_notes(&rows).is_empty());
    }

    #[test]
    fn the_caveat_survives_into_a_successful_row() {
        let row = row_for(BenchTarget::ProcessListUsage, 300, &[400.0], None, None);
        assert_eq!(row.status, BenchStatus::Ok);
        assert!(
            row.note
                .as_deref()
                .is_some_and(|note| note.contains("cpu sample window")),
            "{:?}",
            row.note
        );
    }

    #[test]
    fn every_target_has_a_label_domain_and_budget() {
        for target in BenchTarget::ALL {
            assert!(!target.label().is_empty());
            assert!(
                ["process", "port", "net", "disk", "service"].contains(&target.domain()),
                "{} has an unknown domain",
                target.label()
            );
            assert!(target.budget_ms() > 0.0);
        }
        // The row-proportional reads carry a baseline scale; the fixed-cost
        // reads must not claim one.
        for target in BenchTarget::ALL {
            let has_scale = matches!(target.domain(), "process" | "port")
                && *target != BenchTarget::SocketListening;
            assert_eq!(
                target.baseline_rows().is_some(),
                has_scale,
                "{} has the wrong baseline scale",
                target.label()
            );
        }
        let domains: Vec<&str> = BenchTarget::ALL
            .iter()
            .map(|target| target.domain())
            .collect();
        for domain in ["process", "port", "net", "disk", "service"] {
            assert!(domains.contains(&domain), "no target in {domain}");
        }
    }

    #[test]
    fn bench_covers_every_target_and_counts_real_rows() {
        let report = bench(&populated(), &BenchConfig::default());
        assert_eq!(report.rows.len(), BenchTarget::ALL.len());
        assert_eq!(report.samples, 3);

        let sockets = report
            .rows
            .iter()
            .find(|row| row.target == BenchTarget::SocketList)
            .expect("socket row");
        assert_eq!(sockets.rows, 2);
        assert_eq!(sockets.domain, "port");
        assert_eq!(sockets.status, BenchStatus::Ok);
        assert_eq!(sockets.samples, 3);

        // Nothing here is slow enough to be called slow at these budgets.
        assert!(report.slow_rows().is_empty(), "{:?}", report.slow_rows());
    }

    #[test]
    fn bench_can_be_narrowed_to_one_domain() {
        let report = bench(
            &populated(),
            &BenchConfig {
                domain: Some("port".into()),
                ..Default::default()
            },
        );
        assert_eq!(report.rows.len(), 3);
        assert!(report.rows.iter().all(|row| row.domain == "port"));
    }

    #[test]
    fn a_broken_adapter_degrades_the_row_instead_of_failing_the_run() {
        let stubs = Stubs::new().with_ports(vec![stub_socket(8080, 42, "node")]);
        stubs.port.fail_with(StubFailure::new(
            crate::error::ErrorKind::Unsupported,
            "no socket table here",
        ));
        let report = bench(
            &stubs.context(),
            &BenchConfig {
                samples: 1,
                ..Default::default()
            },
        );
        let failed = report
            .rows
            .iter()
            .filter(|row| row.status == BenchStatus::Failed)
            .collect::<Vec<_>>();
        assert_eq!(failed.len(), 3, "{failed:?}");
        assert!(failed.iter().all(|row| row.domain == "port"));
        assert_eq!(failed[0].note.as_deref(), Some("no socket table here"));
    }

    #[test]
    fn json_names_are_stable_and_snake_cased() {
        let row = row_for(BenchTarget::SocketOwners, 10, &[20.0], None, None);
        let value = serde_json::to_value(&row).expect("json");
        assert_eq!(value["target"], "socket_owners");
        assert_eq!(value["domain"], "port");
        assert_eq!(value["status"], "ok");
        assert_eq!(value["budget_ms"], 1200.0);
        // 20 ms for 10 rows is 2 ms, so 2000 microseconds, per row.
        assert_eq!(value["us_per_row"], 2000.0);
        assert!(
            value.get("caveat").is_none(),
            "the note is spelled `note` in every command"
        );

        let report = BenchReport {
            samples: 1,
            rows: vec![row],
            scale_notes: vec![],
        };
        let text = serde_json::to_string(&report).expect("report json");
        assert!(text.contains("\"samples\":1"));
        assert!(text.contains("\"scale_notes\":[]"));
    }
}
