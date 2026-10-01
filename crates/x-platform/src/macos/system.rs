//! macOS system information: `sysctl` + `sysinfo`.

use crate::common::{cpu_sysinfo, identity};
use crate::sys;
use std::os::raw::{c_int, c_void};
use x_core::error::Result;
use x_core::system::{CpuUsage, MemoryUsage, OsFamily, PressureLevel, SystemInfo, SystemManager};

/// Reads macOS system facts.
#[derive(Debug, Default)]
pub struct MacosSystem;

impl MacosSystem {
    /// Create the adapter.
    pub fn new() -> Self {
        Self
    }
}

impl SystemManager for MacosSystem {
    fn info(&self) -> Result<SystemInfo> {
        let info = sysinfo::System::new_all();
        let cpu = info.cpus();
        let boot_time = sysinfo::System::boot_time();
        let (performance_cores, efficiency_cores) = perf_level_split();

        Ok(SystemInfo {
            os: OsFamily::MacOs,
            os_name: "macOS".into(),
            os_version: product_version(),
            kernel_version: sysinfo::System::kernel_version(),
            arch: std::env::consts::ARCH.into(),
            hostname: sysinfo::System::host_name().unwrap_or_else(|| "unknown".into()),
            cpu_brand: cpu.first().map(|c| c.brand().to_string()),
            cpu_count: cpu.len(),
            physical_cores: sysctl_int("hw.physicalcpu").filter(|c| *c > 0),
            performance_cores,
            efficiency_cores,
            total_memory_bytes: info.total_memory(),
            available_memory_bytes: info.available_memory(),
            uptime_seconds: sysinfo::System::uptime(),
            boot_time: (boot_time > 0).then_some(boot_time),
            current_user: sys::current_user_name(),
            timezone: identity::timezone_name(),
            utc_offset_seconds: identity::utc_offset_seconds(),
            locale: locale(),
            current_shell: identity::login_shell(),
            terminal: identity::terminal_name(),
        })
    }

    fn cpu_usage(&self) -> Result<CpuUsage> {
        let mut usage = cpu_sysinfo::sample(std::time::Duration::from_millis(200));
        // Intel Macs publish a ceiling; Apple silicon does not, and the
        // performance levels only carry names.
        usage.max_frequency_mhz =
            sysctl_int("hw.cpufrequency_max").map(|hertz| hertz as f32 / 1_000_000.0);
        // The SMC sensor that holds the die temperature needs root, and macOS
        // has no scaling governor to name.
        Ok(usage)
    }

    fn memory_usage(&self) -> Result<MemoryUsage> {
        let info = sysinfo::System::new_all();
        let total = info.total_memory();
        let available = info.available_memory();
        let (swap_total, swap_used) = swap_usage();
        Ok(MemoryUsage {
            total_bytes: total,
            used_bytes: total.saturating_sub(available),
            available_bytes: available,
            percent: x_core::percent(total.saturating_sub(available), total),
            swap_total_bytes: swap_total,
            swap_used_bytes: swap_used,
            pressure: memory_pressure(),
        })
    }

    fn current_user(&self) -> Option<String> {
        sys::current_user_name()
    }
}

/// macOS version from `kern.osproductversion`, e.g. `15.6`.
fn product_version() -> String {
    sysctl_string("kern.osproductversion").unwrap_or_else(|| "unknown".into())
}

/// Performance and efficiency physical cores.
///
/// Apple silicon publishes one `hw.perflevel<N>` block per cluster and level 0
/// is always the performance cluster; an Intel Mac reports a single level, and
/// then there is no split to make.
fn perf_level_split() -> (Option<usize>, Option<usize>) {
    let levels = sysctl_int("hw.nperflevels").unwrap_or(0);
    if levels < 2 {
        return (None, None);
    }
    let performance = sysctl_int("hw.perflevel0.physicalcpu");
    let efficiency = (1..levels)
        .filter_map(|level| sysctl_int(&format!("hw.perflevel{level}.physicalcpu")))
        .sum();
    (
        performance.filter(|cores| *cores > 0),
        (efficiency > 0).then_some(efficiency),
    )
}

/// Locale of the running process.
///
/// A terminal session always carries `LANG`; a job started by launchd does not,
/// and then `kern.locale` is what the system was configured with. The `"C"`
/// default is not worth reporting as a locale.
fn locale() -> Option<String> {
    identity::locale_name()
        .or_else(|| sysctl_string("kern.locale").filter(|value| value != "C" && !value.is_empty()))
}

/// Read an integer `sysctl`.
pub fn sysctl_int(name: &str) -> Option<usize> {
    let name = std::ffi::CString::new(name).ok()?;
    let mut value: c_int = 0;
    let mut size = std::mem::size_of::<c_int>() as libc::size_t;
    // SAFETY: `name` is NUL terminated, and `value` plus `size` describe our own
    // storage, which the kernel fills in.
    let rc = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            &mut value as *mut _ as *mut c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    (rc == 0 && value > 0).then_some(value as usize)
}

/// Read a string `sysctl`.
///
/// String sysctls fail with `ENOMEM` when the buffer is too small, so the
/// buffer starts at a generous size and is only retried once to pick up longer
/// values.
pub fn sysctl_string(name: &str) -> Option<String> {
    const INITIAL: usize = 256;
    // `sysctlbyname` expects a NUL terminated C string, not a `&str`.
    let name = std::ffi::CString::new(name).ok()?;
    let mut size = INITIAL;
    for _ in 0..2 {
        let mut buffer = vec![0u8; size];
        // SAFETY: `buffer` is `size` bytes, `size` is updated by the kernel, and
        // `name` is a NUL terminated C string owned by this function.
        let rc = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                buffer.as_mut_ptr() as *mut libc::c_void,
                &mut size as *mut libc::size_t,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc == 0 {
            let bytes: Vec<u8> = buffer
                .iter()
                .take(size.min(buffer.len()))
                .copied()
                .take_while(|b| *b != 0)
                .collect();
            return String::from_utf8(bytes).ok();
        }
        if size <= INITIAL {
            return None;
        }
    }
    None
}

/// Swap capacity and use, read from the `vm.swapusage` sysctl.
///
/// The sysctl is an opaque 32-byte `struct swapusage`, not a string — the
/// `sysctl(8)` CLI formats it into the familiar `total = …M` line itself. The
/// layout is three little-endian `u64` byte counts (total, used, free)
/// followed by flags we do not need.
fn swap_usage() -> (u64, u64) {
    let Some(name) = std::ffi::CString::new("vm.swapusage").ok() else {
        return (0, 0);
    };
    let mut buffer = [0u8; 32];
    let mut size = buffer.len();
    // SAFETY: `name` is NUL terminated and `buffer`/`size` describe our own
    // storage, which the kernel fills in.
    let rc = unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            buffer.as_mut_ptr() as *mut c_void,
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc == 0 && size >= 24 {
        swap_usage_from(&buffer)
    } else {
        (0, 0)
    }
}

/// Pure core of [`swap_usage`]: the first three little-endian `u64`s.
fn swap_usage_from(raw: &[u8]) -> (u64, u64) {
    let read = |index: usize| {
        raw.get(index * 8..index * 8 + 8)
            .map(|chunk| u64::from_le_bytes(chunk.try_into().unwrap_or([0; 8])))
            .unwrap_or(0)
    };
    (read(0), read(1))
}

/// Memory pressure from `kern.memorystatus_vm_pressure_level`.
///
/// The kernel reports 1..=4 (normal, warning, critical, and a fourth level
/// that behaves like critical for our purposes); missing means the sysctl is
/// not present on this kernel, which we surface as `None` rather than a guess.
fn memory_pressure() -> Option<PressureLevel> {
    memory_pressure_from(sysctl_int("kern.memorystatus_vm_pressure_level"))
}

/// Pure core of [`memory_pressure`], injectable for tests.
fn memory_pressure_from(level: Option<usize>) -> Option<PressureLevel> {
    match level? {
        1 => Some(PressureLevel::Normal),
        2 => Some(PressureLevel::Warning),
        _ => Some(PressureLevel::Critical),
    }
}

/// Trait object helper.
pub fn manager() -> std::sync::Arc<dyn SystemManager> {
    std::sync::Arc::new(MacosSystem::new())
}

/// Default adapter.
pub fn as_manager() -> std::sync::Arc<dyn SystemManager> {
    manager()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swap_usage_parses_the_binary_sysctl() {
        // `struct swapusage`: total, used, free as u64 byte counts plus flags.
        let mut raw = vec![0u8; 32];
        raw[0..8].copy_from_slice(&(3072u64 * 1024 * 1024).to_le_bytes());
        raw[8..16].copy_from_slice(&((12.5 * 1024.0 * 1024.0) as u64).to_le_bytes());
        let (total, used) = swap_usage_from(&raw);
        assert_eq!(total, 3072 * 1024 * 1024);
        assert_eq!(used, (12.5 * 1024.0 * 1024.0) as u64);
        assert_eq!(swap_usage_from(&[]), (0, 0));
    }

    #[test]
    fn pressure_levels_map_from_the_kernel_scale() {
        assert_eq!(memory_pressure_from(Some(1)), Some(PressureLevel::Normal));
        assert_eq!(memory_pressure_from(Some(2)), Some(PressureLevel::Warning));
        assert_eq!(memory_pressure_from(Some(4)), Some(PressureLevel::Critical));
        assert_eq!(memory_pressure_from(None), None);
    }

    #[test]
    fn product_version_looks_like_a_version() {
        let version = product_version();
        assert!(
            version.split('.').count() >= 2,
            "unexpected product version: {version}"
        );
    }

    #[test]
    fn sysctl_integers_are_readable() {
        let cores = sysctl_int("hw.physicalcpu").expect("hw.physicalcpu");
        assert!(cores >= 1, "unexpected core count: {cores}");
        assert!(cores <= sysctl_int("hw.logicalcpu").unwrap_or(cores));
    }

    #[test]
    fn sysctl_strings_are_readable() {
        assert!(sysctl_string("kern.osrelease").is_some());
        assert!(sysctl_string("kern.definitely_missing").is_none());
    }

    #[test]
    fn system_info_is_populated() {
        let info = MacosSystem::new().info().expect("info");
        assert_eq!(info.os, OsFamily::MacOs);
        assert!(info.cpu_count > 0, "at least one CPU");
        let physical = info.physical_cores.unwrap_or(0);
        assert!(
            physical > 0 && physical <= info.cpu_count,
            "physical {physical} vs logical {}",
            info.cpu_count
        );
        assert!(info.total_memory_bytes > 0);
        assert!(!info.hostname.is_empty());
        assert_eq!(info.arch, std::env::consts::ARCH);
    }

    #[test]
    fn cpu_usage_is_in_range() {
        let usage = MacosSystem::new().cpu_usage().expect("cpu");
        assert!((0.0..=100.0).contains(&usage.total_percent));
        assert!(
            !usage.per_core_percent.is_empty(),
            "per core usage must be reported"
        );
        assert!(usage
            .per_core_percent
            .iter()
            .all(|v| (0.0..=100.0).contains(v)));
        assert!(usage.load_average.is_some(), "getloadavg always works");
        assert!(usage.governor.is_none(), "macOS has no scaling governor");
        if let Some(frequency) = usage.frequency_mhz {
            assert!(frequency > 0.0, "a reported clock must be positive");
        }
    }

    #[test]
    fn the_core_split_is_whole_or_absent() {
        let info = MacosSystem::new().info().expect("info");
        match (info.performance_cores, info.efficiency_cores) {
            (Some(performance), Some(efficiency)) => {
                assert!(performance >= 1 && efficiency >= 1);
                assert!(
                    performance + efficiency <= info.cpu_count,
                    "p{performance} e{efficiency} of {}",
                    info.cpu_count
                );
            }
            (None, None) => {}
            half => panic!("half a performance split: {half:?}"),
        }
    }

    #[test]
    fn memory_usage_is_consistent() {
        let usage = MacosSystem::new().memory_usage().expect("memory");
        assert_eq!(usage.used_bytes + usage.available_bytes, usage.total_bytes);
        assert!(usage.total_bytes > 0);
    }
}
