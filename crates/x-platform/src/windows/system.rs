//! Windows system adapter: `sysinfo` for CPU, memory and versions, the logical
//! processor information API for the physical core count.

use super::buffer::AlignedBuffer;
use crate::sys;
use windows_sys::Win32::System::SystemInformation::{
    GetLogicalProcessorInformation, GetLogicalProcessorInformationEx, RelationProcessorCore,
    SYSTEM_LOGICAL_PROCESSOR_INFORMATION,
};
use x_core::error::Result;
use x_core::system::{CpuUsage, MemoryUsage, OsFamily, SystemInfo, SystemManager};

/// Reads Windows system facts.
#[derive(Debug, Default)]
pub struct WindowsSystem;

impl WindowsSystem {
    /// Create the adapter.
    pub fn new() -> Self {
        Self
    }
}

impl SystemManager for WindowsSystem {
    fn info(&self) -> Result<SystemInfo> {
        let info = sysinfo::System::new_all();
        let cpu = info.cpus();
        let boot_time = sysinfo::System::boot_time();

        Ok(SystemInfo {
            os: OsFamily::Windows,
            os_name: sysinfo::System::name().unwrap_or_else(|| "Windows".to_string()),
            // `long_os_version` is the marketing name plus the build number,
            // which is what `os_version` means in the unified model.
            os_version: sysinfo::System::long_os_version()
                .or_else(sysinfo::System::os_version)
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

/// Physical cores from the logical processor information API.
///
/// Windows reports one `RelationProcessorCore` record per physical core, which
/// is exactly the number the unified model wants. The Ex variant is preferred
/// because it is correct on machines with more than 64 logical processors; the
/// flat variant is the fallback.
pub fn physical_cores() -> Option<usize> {
    // SAFETY: both calls pass the size the API writes back, and the buffer is
    // only read for the records the API says it filled.
    unsafe {
        let mut size: u32 = 0;
        GetLogicalProcessorInformationEx(RelationProcessorCore, std::ptr::null_mut(), &mut size);
        if size as usize >= std::mem::size_of::<u32>() {
            let mut buffer = vec![0u8; size as usize];
            if GetLogicalProcessorInformationEx(
                RelationProcessorCore,
                buffer.as_mut_ptr().cast(),
                &mut size,
            ) != 0
            {
                let mut cursor = 0usize;
                let mut cores = 0usize;
                while cursor + std::mem::size_of::<u32>() <= buffer.len() {
                    let length =
                        u32::from_le_bytes(buffer[cursor..cursor + 4].try_into().expect("4 bytes"))
                            as usize;
                    if length == 0 || cursor + length > buffer.len() {
                        break;
                    }
                    cores += 1;
                    cursor += length;
                }
                if cores > 0 {
                    return Some(cores);
                }
            }
        }

        size = 0;
        GetLogicalProcessorInformation(std::ptr::null_mut(), &mut size);
        if size == 0 {
            return None;
        }
        let record = std::mem::size_of::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION>();
        let mut buffer = AlignedBuffer::zeroed(size as usize);
        if GetLogicalProcessorInformation(
            buffer
                .as_mut_ptr()
                .cast::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION>(),
            &mut size,
        ) == 0
        {
            return None;
        }
        (0..(size as usize / record))
            .filter_map(|index| {
                let start = index * record;
                // SAFETY: `start` is inside the buffer and the record size is
                // exactly one `SYSTEM_LOGICAL_PROCESSOR_INFORMATION`. Records are
                // packed, so the read must not assume alignment.
                let entry = buffer.read_at::<SYSTEM_LOGICAL_PROCESSOR_INFORMATION>(start);
                (entry.Relationship == RelationProcessorCore).then_some(())
            })
            .count()
            .into()
    }
}

/// Trait object helper.
pub fn manager() -> std::sync::Arc<dyn SystemManager> {
    std::sync::Arc::new(WindowsSystem::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_cores_never_exceed_the_logical_count() {
        let logical = sysinfo::System::new_all().cpus().len().max(1);
        let physical = physical_cores().expect("physical cores");
        assert!(
            (1..=logical).contains(&physical),
            "{physical} physical vs {logical} logical"
        );
    }

    #[test]
    fn system_info_is_populated() {
        let info = WindowsSystem::new().info().expect("info");
        assert_eq!(info.os, OsFamily::Windows);
        assert!(info.cpu_count > 0);
        assert!(info.total_memory_bytes > 0);
        assert!(!info.hostname.is_empty());
    }

    #[test]
    fn memory_and_cpu_are_in_range() {
        let system = WindowsSystem::new();
        let memory = system.memory_usage().expect("memory");
        assert!(memory.percent >= 0.0 && memory.percent <= 100.0);

        let cpu = system.cpu_usage().expect("cpu");
        assert!(cpu.total_percent >= 0.0 && cpu.total_percent <= 100.0);
    }
}
