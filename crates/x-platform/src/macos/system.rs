//! macOS system information: `sysctl` + `sysinfo`.

use crate::sys;
use std::os::raw::{c_int, c_void};
use x_core::error::Result;
use x_core::system::{CpuUsage, MemoryUsage, OsFamily, SystemInfo, SystemManager};

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
            total_memory_bytes: info.total_memory(),
            available_memory_bytes: info.available_memory(),
            uptime_seconds: sysinfo::System::uptime(),
            boot_time: (boot_time > 0).then_some(boot_time),
            current_user: sys::current_user_name(),
        })
    }

    fn cpu_usage(&self) -> Result<CpuUsage> {
        // sysinfo needs two samples separated by an interval to report CPU
        // usage; one refresh is only good enough for the first dashboard frame.
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
        let info = sysinfo::System::new_all();
        let total = info.total_memory();
        let available = info.available_memory();
        Ok(MemoryUsage {
            total_bytes: total,
            used_bytes: total.saturating_sub(available),
            available_bytes: available,
            percent: x_core::percent(total.saturating_sub(available), total),
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
    }

    #[test]
    fn memory_usage_is_consistent() {
        let usage = MacosSystem::new().memory_usage().expect("memory");
        assert_eq!(usage.used_bytes + usage.available_bytes, usage.total_bytes);
        assert!(usage.total_bytes > 0);
    }
}
