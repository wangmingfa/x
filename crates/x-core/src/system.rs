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
}

/// Live CPU utilization. Empty per-core lists mean "not measured".
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CpuUsage {
    /// Aggregate usage percentage across all cores, `0.0..=100.0`.
    pub total_percent: f32,
    /// Per core usage, ordered by core index.
    pub per_core_percent: Vec<f32>,
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
}
