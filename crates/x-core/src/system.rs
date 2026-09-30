//! Unified system information: OS, CPU, memory, uptime.

use serde::{Deserialize, Serialize};

/// Operating system family. x never branches on this: adapters do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OsFamily {
    /// Windows.
    Windows,
    /// Linux.
    Linux,
    /// macOS / iOS.
    MacOs,
    /// BSD family.
    Bsd,
    /// Anything else, and the default for values that were never set.
    #[default]
    Other,
}

impl OsFamily {
    /// Canonical lowercase name.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Windows => "windows",
            Self::Linux => "linux",
            Self::MacOs => "macos",
            Self::Bsd => "bsd",
            Self::Other => "other",
        }
    }
}

/// Static and dynamic system facts.
///
/// [`Default`] yields an all-empty document; it exists for tests and for
/// frontends that need a placeholder value, not for real data.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SystemInfo {
    /// OS family.
    pub os: OsFamily,
    /// Marketing name, e.g. `macOS`, `Ubuntu`.
    pub os_name: String,
    /// Kernel or OS version, e.g. `15.6`.
    pub os_version: String,
    /// Kernel version string (distinct from the marketing version on Windows).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kernel_version: Option<String>,
    /// CPU architecture, e.g. `arm64`, `x86_64`.
    pub arch: String,
    /// Host name.
    pub hostname: String,
    /// CPU brand string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_brand: Option<String>,
    /// Number of logical CPUs.
    pub cpu_count: usize,
    /// Physical core count when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub physical_cores: Option<usize>,
    /// Performance ("big") cores on platforms that publish the split: macOS
    /// reports it per performance level, Windows per efficiency class, Linux
    /// infers it from the per-cluster maximum frequency. `None` means the
    /// layout is homogeneous or hidden.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub performance_cores: Option<usize>,
    /// Efficiency ("little") cores, the counterpart of
    /// [`SystemInfo::performance_cores`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub efficiency_cores: Option<usize>,
    /// Total physical memory in bytes.
    pub total_memory_bytes: u64,
    /// Available physical memory in bytes.
    pub available_memory_bytes: u64,
    /// System uptime in seconds.
    pub uptime_seconds: u64,
    /// Unix boot time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boot_time: Option<u64>,
    /// Current user name, when the platform exposes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_user: Option<String>,
    /// Time zone name as the platform spells it: an IANA id on Unix
    /// (`Asia/Shanghai`), the Windows time zone key (`China Standard Time`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    /// Offset between local time and UTC right now, in seconds. This is what
    /// turns [`SystemInfo::boot_time`] into a readable local timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub utc_offset_seconds: Option<i64>,
    /// Locale of the running process, e.g. `zh_CN.UTF-8`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    /// Shell of the current user. On Windows this is the shell the OS would
    /// hand out (`COMSPEC`), not necessarily the one hosting `x`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_shell: Option<String>,
    /// Terminal emulator `x` is running inside, when it can be identified.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
}

/// Live CPU utilization. Empty per-core lists mean "not measured", `None`
/// detail fields mean "this platform does not expose it".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CpuUsage {
    /// Aggregate usage percentage across all cores, `0.0..=100.0`.
    pub total_percent: f32,
    /// Per core usage, ordered by core index.
    pub per_core_percent: Vec<f32>,
    /// Runnable-task averages. Unix kernels publish them directly; Windows
    /// smooths `\System\Cpu Queue Length` into the same shape, and that sampler
    /// needs a few seconds before it reports anything but zero.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub load_average: Option<LoadAverage>,
    /// Current clock of the cores, averaged in MHz.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_mhz: Option<f32>,
    /// Highest clock the hardware advertises, in MHz.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_frequency_mhz: Option<f32>,
    /// Per core clock in MHz, aligned with [`CpuUsage::per_core_percent`], where
    /// a core the kernel would not report holds `0.0`. Empty means no core at all
    /// reported a clock.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub per_core_frequency_mhz: Vec<f32>,
    /// Package temperature in degrees Celsius. Only platforms with a
    /// user-readable sensor fill this; the others report `None`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature_celsius: Option<f32>,
    /// Frequency-scaling policy (`scaling_governor`). Linux only; macOS and
    /// Windows keep the decision inside the power manager where it is not
    /// readable as a string.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub governor: Option<String>,
}

/// Load average: how many tasks were runnable on average over 1, 5 and 15
/// minutes. Compared against the core count it is the queueing signal behind a
/// busy CPU number.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct LoadAverage {
    /// Average over the last minute.
    pub one: f32,
    /// Average over the last five minutes.
    pub five: f32,
    /// Average over the last fifteen minutes.
    pub fifteen: f32,
}

impl std::fmt::Display for LoadAverage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{:.2} {:.2} {:.2}",
            self.one, self.five, self.fifteen
        )
    }
}

/// Kernel-reported memory pressure, in rising severity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum PressureLevel {
    /// The kernel reclaims memory without disturbing workloads.
    #[default]
    Normal,
    /// Paging and compression are costing noticeable time.
    Warning,
    /// The system is thrashing; allocations stall.
    Critical,
}

impl std::fmt::Display for PressureLevel {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            PressureLevel::Normal => "normal",
            PressureLevel::Warning => "warning",
            PressureLevel::Critical => "critical",
        };
        formatter.write_str(name)
    }
}

/// Live memory utilization. Zeroed values mean "not measured".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MemoryUsage {
    /// Total physical memory.
    pub total_bytes: u64,
    /// In use memory.
    pub used_bytes: u64,
    /// Available memory.
    pub available_bytes: u64,
    /// Usage percentage.
    pub percent: f32,
    /// Swap (macOS/Linux) or page file (Windows) capacity. Zero means the
    /// platform has none configured or does not expose it.
    #[serde(skip_serializing_if = "is_zero")]
    pub swap_total_bytes: u64,
    /// Swap or page file space currently in use.
    #[serde(skip_serializing_if = "is_zero")]
    pub swap_used_bytes: u64,
    /// Kernel memory-pressure level. `None` when the platform does not
    /// publish one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pressure: Option<PressureLevel>,
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

/// System capability.
pub trait SystemManager: Send + Sync {
    /// Static and dynamic system facts.
    fn info(&self) -> crate::error::Result<SystemInfo>;

    /// Current CPU utilization.
    fn cpu_usage(&self) -> crate::error::Result<CpuUsage>;

    /// Current memory utilization.
    fn memory_usage(&self) -> crate::error::Result<MemoryUsage> {
        let info = self.info()?;
        let total = info.total_memory_bytes;
        let available = info.available_memory_bytes;
        Ok(MemoryUsage {
            total_bytes: total,
            used_bytes: total.saturating_sub(available),
            available_bytes: available,
            percent: percent(total.saturating_sub(available), total),
            swap_total_bytes: 0,
            swap_used_bytes: 0,
            pressure: None,
        })
    }

    /// Seconds since boot.
    fn uptime(&self) -> crate::error::Result<u64> {
        self.info().map(|i| i.uptime_seconds)
    }

    /// Name of the user x is running as, used for privilege guidance.
    fn current_user(&self) -> Option<String> {
        None
    }
}

/// Safe percentage computation, returns `0.0` instead of dividing by zero.
pub fn percent(used: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (used as f64 / total as f64 * 100.0) as f32
    }
}

/// Human readable byte size using binary units, e.g. `36.4 GB`.
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{} {}", bytes, UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Human readable duration, e.g. `3d 12h`, `4m 12s`.
pub fn format_duration(seconds: u64) -> String {
    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3_600;
    let minutes = (seconds % 3_600) / 60;
    let secs = seconds % 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {secs}s")
    } else {
        format!("{secs}s")
    }
}

/// Wall clock for a Unix timestamp at `offset` seconds from UTC, e.g.
/// `2026-09-28 09:14:02 +08:00`.
///
/// The calendar conversion is done here instead of by the platform layer so the
/// unified model keeps rendering its own numbers, and so the whole function is
/// testable without a time zone database.
pub fn format_timestamp(unix_seconds: u64, offset_seconds: i64) -> String {
    let local = unix_seconds as i64 + offset_seconds;
    let days = local.div_euclid(86_400);
    let time = local.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let (sign, hours, minutes) = split_offset(offset_seconds);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} {sign}{hours:02}:{minutes:02}",
        time / 3_600,
        (time % 3_600) / 60,
        time % 60,
    )
}

/// Days since the Unix epoch into a proleptic Gregorian date.
///
/// Howard Hinnant's `civil_from_days`: integer division that rounds toward
/// negative infinity is what makes it correct for dates before 1970 too.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_probe = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_probe + 2) / 5 + 1;
    let month = month_probe + if month_probe < 10 { 3 } else { -9 };
    (year + i64::from(month <= 2), month, day)
}

/// `(sign, hours, minutes)` of an offset expressed in seconds.
fn split_offset(offset_seconds: i64) -> (char, i64, i64) {
    let sign = if offset_seconds < 0 { '-' } else { '+' };
    let abs = offset_seconds.abs();
    (sign, abs / 3_600, (abs % 3_600) / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_formatting() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KB");
        assert_eq!(format_bytes(36 * 1024 * 1024 * 1024), "36.0 GB");
    }

    #[test]
    fn duration_formatting() {
        assert_eq!(format_duration(30), "30s");
        assert_eq!(format_duration(252), "4m 12s");
        assert_eq!(format_duration(3 * 86_400 + 12 * 3_600), "3d 12h");
    }

    #[test]
    fn percent_is_zero_safe() {
        assert_eq!(percent(1, 0), 0.0);
        assert_eq!(percent(50, 200), 25.0);
    }

    #[test]
    fn load_averages_print_in_three_columns() {
        let load = LoadAverage {
            one: 0.5,
            five: 1.0,
            fifteen: 10.25,
        };
        assert_eq!(load.to_string(), "0.50 1.00 10.25");
        assert_eq!(LoadAverage::default().to_string(), "0.00 0.00 0.00");
    }

    #[test]
    fn timestamps_render_in_the_requested_offset() {
        // 2025-01-01T00:00:00Z
        let epoch = 1_735_689_600;
        assert_eq!(format_timestamp(epoch, 0), "2025-01-01 00:00:00 +00:00");
        assert_eq!(
            format_timestamp(epoch, 8 * 3_600),
            "2025-01-01 08:00:00 +08:00"
        );
        assert_eq!(
            format_timestamp(epoch, -5 * 3_600),
            "2024-12-31 19:00:00 -05:00"
        );
        // A half-hour zone, as India and Iran use.
        assert_eq!(format_timestamp(epoch, 5_580), "2025-01-01 01:33:00 +01:33");
    }

    #[test]
    fn timestamps_handle_leap_days_and_epoch_crossings() {
        // 2024-02-29T00:00:00Z must not turn into March.
        assert_eq!(
            format_timestamp(1_709_164_800, 0),
            "2024-02-29 00:00:00 +00:00"
        );
        assert_eq!(format_timestamp(0, 0), "1970-01-01 00:00:00 +00:00");
        assert_eq!(format_timestamp(0, -3_600), "1969-12-31 23:00:00 -01:00");
    }
}
