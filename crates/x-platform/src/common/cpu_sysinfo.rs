//! The CPU reading every platform shares: a two-sample `sysinfo` interval.
//!
//! Usage is a delta between samples, so a single refresh reports zero. All three
//! kernels also expose the live per-core clock through `sysinfo`, which leaves
//! adapters one job: adding whatever only their OS knows.

use std::time::Duration;

use x_core::system::{CpuUsage, LoadAverage};

/// Sample CPU usage and clocks over `interval`, with the load average.
///
/// Fields with no cross-platform source start empty and are filled by the
/// adapter: `max_frequency_mhz`, `temperature_celsius`, `governor`.
pub fn sample(interval: Duration) -> CpuUsage {
    let refresh = sysinfo::CpuRefreshKind::nothing()
        .with_cpu_usage()
        .with_frequency();
    let mut system = sysinfo::System::new();
    system.refresh_cpu_specifics(refresh);
    std::thread::sleep(interval);
    system.refresh_cpu_specifics(refresh);

    let load = sysinfo::System::load_average();
    // Aligned with the cores, so a frontend can show one row per core; a core
    // whose kernel reports no clock holds 0.0. A machine where no core reported
    // keeps the list empty, which is the model's "not measured" signal.
    let per_core_frequency_mhz: Vec<f32> = system
        .cpus()
        .iter()
        .map(|core| core.frequency() as f32)
        .collect();
    let frequency_mhz = average(&per_core_frequency_mhz);
    let per_core_frequency_mhz = frequency_mhz
        .map(|_| per_core_frequency_mhz)
        .unwrap_or_default();

    CpuUsage {
        total_percent: system.global_cpu_usage().clamp(0.0, 100.0),
        per_core_percent: system
            .cpus()
            .iter()
            .map(|core| core.cpu_usage().clamp(0.0, 100.0))
            .collect(),
        load_average: Some(LoadAverage {
            one: load.one as f32,
            five: load.five as f32,
            fifteen: load.fifteen as f32,
        }),
        frequency_mhz,
        max_frequency_mhz: None,
        per_core_frequency_mhz,
        temperature_celsius: None,
        governor: None,
    }
}

/// Mean of the positive readings, `None` when there were none.
pub fn average(values: &[f32]) -> Option<f32> {
    let measured: Vec<f32> = values
        .iter()
        .copied()
        .filter(|value| *value > 0.0)
        .collect();
    if measured.is_empty() {
        return None;
    }
    Some(measured.iter().sum::<f32>() / measured.len() as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn averaging_ignores_cores_that_reported_nothing() {
        assert_eq!(average(&[]), None);
        assert_eq!(average(&[0.0, 0.0]), None);
        assert_eq!(average(&[0.0, 1200.0]), Some(1200.0));
        assert_eq!(average(&[1200.0, 2400.0]), Some(1800.0));
    }

    #[test]
    fn a_sample_reports_usage_and_load_everywhere() {
        let usage = sample(Duration::from_millis(50));
        assert!(!usage.per_core_percent.is_empty(), "one row per core");
        assert!(usage.load_average.is_some(), "load average always exists");
        assert!(
            usage.max_frequency_mhz.is_none(),
            "the floor adds no detail"
        );
        assert!(usage.governor.is_none());
        assert!(usage.temperature_celsius.is_none());
        if let Some(frequency) = usage.frequency_mhz {
            assert!(frequency > 0.0);
        }
    }
}
