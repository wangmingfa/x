//! Linux system adapter: `sysinfo` for CPU and memory, `/etc/os-release` for the
//! distribution name, `/proc/cpuinfo` for the physical core count.

use crate::sys;
use x_core::error::Result;
use x_core::system::{CpuUsage, MemoryUsage, OsFamily, SystemInfo, SystemManager};

/// Reads Linux system facts.
#[derive(Debug, Default)]
pub struct LinuxSystem;

impl LinuxSystem {
    /// Create the adapter.
    pub fn new() -> Self {
        Self
    }
}

impl SystemManager for LinuxSystem {
    fn info(&self) -> Result<SystemInfo> {
        let info = sysinfo::System::new_all();
        let cpu = info.cpus();
        let boot_time = sysinfo::System::boot_time();

        Ok(SystemInfo {
            os: OsFamily::Linux,
            os_name: os_release()
                .map(|release| release.pretty_name)
                .unwrap_or_else(|| "Linux".to_string()),
            os_version: os_release()
                .map(|release| release.version)
                .unwrap_or_default(),
            kernel_version: sysinfo::System::kernel_version(),
            arch: std::env::consts::ARCH.into(),
            hostname: sysinfo::System::host_name().unwrap_or_else(|| "unknown".into()),
            cpu_brand: cpu
                .first()
                .map(|c| c.brand().to_string())
                .filter(|brand| !brand.is_empty()),
            cpu_count: cpu.len(),
            physical_cores: Some(physical_cores().unwrap_or(cpu.len())),
            total_memory_bytes: info.total_memory(),
            available_memory_bytes: info.available_memory(),
            uptime_seconds: sysinfo::System::uptime(),
            boot_time: (boot_time > 0).then_some(boot_time),
            current_user: sys::current_user_name(),
        })
    }

    fn cpu_usage(&self) -> Result<CpuUsage> {
        // sysinfo computes usage from the delta between two refreshes.
        let mut system = sysinfo::System::new();
        system.refresh_cpu_usage();
        std::thread::sleep(std::time::Duration::from_millis(200));
        system.refresh_cpu_usage();
        Ok(CpuUsage {
            total_percent: system.global_cpu_usage().clamp(0.0, 100.0),
            per_core_percent: system
                .cpus()
                .iter()
                .map(|c| c.cpu_usage().clamp(0.0, 100.0))
                .collect(),
        })
    }

    fn memory_usage(&self) -> Result<MemoryUsage> {
        // `/proc/meminfo` is authoritative; sysinfo already parses it.
        let info = sysinfo::System::new_all();
        let total = info.total_memory();
        let available = info.available_memory();
        Ok(MemoryUsage {
            total_bytes: total,
            used_bytes: total.saturating_sub(available),
            available_bytes: available,
            percent: percent(total.saturating_sub(available), total),
        })
    }
}

fn percent(part: u64, whole: u64) -> f32 {
    if whole == 0 {
        0.0
    } else {
        (part as f64 / whole as f64 * 100.0) as f32
    }
}

/// Distribution identity from `/etc/os-release`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OsRelease {
    /// `PRETTY_NAME`, falling back to `NAME`.
    pub pretty_name: String,
    /// `VERSION_ID`.
    pub version: String,
    /// `ID`.
    pub id: String,
}

/// Parse the `KEY=value` lines of an `os-release` file.
pub fn parse_os_release(raw: &str) -> OsRelease {
    let mut name = String::new();
    let mut pretty = String::new();
    let mut version = String::new();
    let mut id = String::new();

    for line in raw.lines() {
        let line = line.trim();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        // Values may be quoted; `os-release` forbids escapes inside them.
        let value = value.trim().trim_matches('"').to_string();
        match key.trim() {
            "NAME" => name = value,
            "PRETTY_NAME" => pretty = value,
            "VERSION_ID" => version = value,
            "ID" => id = value,
            _ => {}
        }
    }

    let pretty_name = if pretty.is_empty() { name } else { pretty };
    OsRelease {
        pretty_name: if pretty_name.is_empty() {
            "Linux".to_string()
        } else {
            pretty_name
        },
        version,
        id: if id.is_empty() {
            "linux".to_string()
        } else {
            id
        },
    }
}

/// Read `/etc/os-release`, falling back to `/usr/lib/os-release`.
pub fn os_release() -> Option<OsRelease> {
    ["/etc/os-release", "/usr/lib/os-release"]
        .into_iter()
        .find_map(|path| std::fs::read_to_string(path).ok())
        .map(|raw| parse_os_release(&raw))
}

/// Physical cores from `/proc/cpuinfo`.
///
/// x86 exposes a `physical id` per socket and a `core id` per core, so counting
/// distinct pairs is exact. Kernels without those fields report `cpu cores`
/// instead, and a container that hides both falls back to the logical count.
pub fn physical_cores() -> Option<usize> {
    if let Ok(raw) = std::fs::read_to_string("/proc/cpuinfo") {
        let mut sockets = std::collections::HashSet::new();
        let mut socket: Option<String> = None;
        for line in raw.lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            match key.trim() {
                "physical id" => socket = Some(value.trim().to_string()),
                "core id" => {
                    if let Some(socket) = socket.clone() {
                        sockets.insert((socket, value.trim().to_string()));
                    }
                }
                _ => {}
            }
        }
        if !sockets.is_empty() {
            return Some(sockets.len());
        }
    }
    cpuinfo_field("cpu cores")
        .and_then(|value| value.parse().ok())
        .filter(|cores: &usize| *cores > 0)
}

/// First value of a `key : value` field in `/proc/cpuinfo`.
pub fn cpuinfo_field(key: &str) -> Option<String> {
    let raw = std::fs::read_to_string("/proc/cpuinfo").ok()?;
    raw.lines()
        .find_map(|line| line.split_once(':'))
        .filter(|(name, _)| name.trim() == key)
        .map(|(_, value)| value.trim().to_string())
}

/// Trait object helper.
pub fn manager() -> std::sync::Arc<dyn SystemManager> {
    std::sync::Arc::new(LinuxSystem::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn os_release_parsing_handles_quotes_and_comments() {
        let raw = "NAME=\"Ubuntu\"\nVERSION_ID=\"22.04\"\nID=ubuntu\nPRETTY_NAME=\"Ubuntu 22.04.3 LTS\"\n# comment\nBROKEN\n";
        let release = parse_os_release(raw);
        assert_eq!(release.id, "ubuntu");
        assert_eq!(release.version, "22.04");
        assert_eq!(release.pretty_name, "Ubuntu 22.04.3 LTS");
    }

    #[test]
    fn os_release_parsing_falls_back_to_name() {
        let release = parse_os_release("NAME=Alpine Linux\nID=alpine\n");
        assert_eq!(release.pretty_name, "Alpine Linux");
    }

    #[test]
    fn physical_cores_never_exceed_the_logical_count() {
        let logical = sysinfo::System::new_all().cpus().len();
        let physical = physical_cores().expect("physical cores");
        assert!(
            physical >= 1 && physical <= logical.max(1),
            "{physical}/{logical}"
        );
    }

    #[test]
    fn system_info_is_populated() {
        let info = LinuxSystem::new().info().expect("info");
        assert_eq!(info.os, OsFamily::Linux);
        assert!(info.cpu_count > 0);
        assert!(info.total_memory_bytes > 0);
        assert!(!info.os_name.is_empty());
        assert!(
            info.kernel_version
                .as_deref()
                .is_some_and(|v| !v.is_empty()),
            "kernel version missing"
        );
    }

    #[test]
    fn memory_and_cpu_are_in_range() {
        let system = LinuxSystem::new();
        let memory = system.memory_usage().expect("memory");
        assert!(memory.percent >= 0.0 && memory.percent <= 100.0);
        assert!(memory.used_bytes <= memory.total_bytes);

        let cpu = system.cpu_usage().expect("cpu");
        assert!(cpu.total_percent >= 0.0 && cpu.total_percent <= 100.0);
        assert!(!cpu.per_core_percent.is_empty());
    }
}
