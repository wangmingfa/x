//! Windows system adapter: `sysinfo` for CPU, memory and versions, the logical
//! processor information API for the physical core count and the efficiency
//! classes, and the power manager for the clocks.

use super::buffer::AlignedBuffer;
use crate::common::{cpu_sysinfo, identity};
use crate::sys;
use windows_sys::Win32::System::Power::{
    CallNtPowerInformation, ProcessorInformation, PROCESSOR_POWER_INFORMATION,
};
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
        let (performance_cores, efficiency_cores) =
            split_by_efficiency_classes(&core_efficiency_classes());

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
            performance_cores,
            efficiency_cores,
            total_memory_bytes: info.total_memory(),
            available_memory_bytes: info.available_memory(),
            uptime_seconds: sysinfo::System::uptime(),
            boot_time: (boot_time > 0).then_some(boot_time),
            current_user: sys::current_user_name(),
            timezone: identity::timezone_name(),
            utc_offset_seconds: identity::utc_offset_seconds(),
            locale: identity::locale_name(),
            current_shell: identity::login_shell(),
            terminal: identity::terminal_name(),
        })
    }

    fn cpu_usage(&self) -> Result<CpuUsage> {
        let mut usage = cpu_sysinfo::sample(std::time::Duration::from_millis(200));
        // The power manager is the authority for both clocks on Windows: it knows
        // the ceiling `sysinfo` never sees, and reports the live clock per
        // logical processor rather than as a per-core frequency hint.
        let clocks = processor_clocks(usage.per_core_percent.len());
        usage.frequency_mhz = clocks.current_mhz().or(usage.frequency_mhz);
        usage.max_frequency_mhz = clocks.max_mhz;
        // Temperature hides behind WMI or the embedded controller, and a power
        // scheme is a GUID rather than a governor name, so neither is reported.
        Ok(usage)
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
            let mut buffer = AlignedBuffer::zeroed(size as usize);
            if GetLogicalProcessorInformationEx(
                RelationProcessorCore,
                buffer.as_mut_ptr().cast(),
                &mut size,
            ) != 0
            {
                // One record per physical core, for as long as the buffer holds.
                let records = processor_records(buffer.bytes()).len();
                if records > 0 {
                    return Some(records);
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

/// `(offset, size)` of every record in a `...InformationEx` buffer.
///
/// Records are variable length, and the size sits at offset 4, *after* the
/// `Relationship` field, so the walk must not assume a fixed stride and must not
/// mistake a `RelationProcessorCore` of `0` for a zero-length record.
pub fn processor_records(bytes: &[u8]) -> Vec<(usize, usize)> {
    let mut records = Vec::new();
    let mut cursor = 0usize;
    while cursor + 8 <= bytes.len() {
        let size = u32::from_le_bytes(
            bytes[cursor + 4..cursor + 8]
                .try_into()
                .expect("four bytes"),
        ) as usize;
        if size < 8 || cursor + size > bytes.len() {
            break;
        }
        records.push((cursor, size));
        cursor += size;
    }
    records
}

/// The efficiency class Windows assigns to each physical core.
///
/// The processor relationship starts 8 bytes into the record and
/// `EfficiencyClass` is its second byte. Class 0 is the highest-performance rank,
/// so on a hybrid part the efficiency cores carry a larger number.
pub fn core_efficiency_classes() -> Vec<u8> {
    // SAFETY: the two-call pattern hands the API the buffer size it asked for,
    // and only the bytes it reports are read back.
    unsafe {
        let mut size: u32 = 0;
        GetLogicalProcessorInformationEx(RelationProcessorCore, std::ptr::null_mut(), &mut size);
        if size == 0 {
            return Vec::new();
        }
        let mut buffer = AlignedBuffer::zeroed(size as usize);
        if GetLogicalProcessorInformationEx(
            RelationProcessorCore,
            buffer.as_mut_ptr().cast(),
            &mut size,
        ) == 0
        {
            return Vec::new();
        }
        processor_records(buffer.bytes())
            .into_iter()
            .filter_map(|(offset, _)| buffer.bytes().get(offset + 9).copied())
            .collect()
    }
}

/// `(performance, efficiency)` core counts from the efficiency classes.
///
/// A part where every core shares one class is homogeneous, and there is no
/// split to report.
pub fn split_by_efficiency_classes(classes: &[u8]) -> (Option<usize>, Option<usize>) {
    let Some(&highest_rank) = classes.iter().min() else {
        return (None, None);
    };
    if classes.iter().all(|class| *class == highest_rank) {
        return (None, None);
    }
    let performance = classes
        .iter()
        .filter(|class| **class == highest_rank)
        .count();
    (Some(performance), Some(classes.len() - performance))
}

/// The highest clock the power manager advertises, in MHz.
///
/// `CallNtPowerInformation(ProcessorInformation)` is the only user-mode API that
/// knows both the ceiling of the part and the clock it holds right now.
pub fn processor_clocks(processors: usize) -> ProcessorClocks {
    let mut clocks = ProcessorClocks::default();
    if processors == 0 {
        return clocks;
    }
    let record = std::mem::size_of::<PROCESSOR_POWER_INFORMATION>();
    // SAFETY: the buffer is exactly `record` bytes per logical processor, which
    // is the array shape the level documents.
    unsafe {
        let mut buffer = AlignedBuffer::zeroed(record * processors);
        let status = CallNtPowerInformation(
            ProcessorInformation,
            std::ptr::null(),
            0,
            buffer.as_mut_ptr(),
            (record * processors) as u32,
        );
        if status != 0 {
            return clocks;
        }
        for index in 0..processors {
            // SAFETY: `index * record` stays inside the buffer the API filled.
            let entry = buffer.read_at::<PROCESSOR_POWER_INFORMATION>(index * record);
            if entry.MaxMhz > 0 {
                let mhz = entry.MaxMhz as f32;
                clocks.max_mhz = Some(clocks.max_mhz.map_or(mhz, |current| current.max(mhz)));
            }
            if entry.CurrentMhz > 0 {
                clocks.current_sum += entry.CurrentMhz as f32;
                clocks.current_count += 1;
            }
        }
    }
    clocks
}

/// What the power manager says about the clocks of the machine.
#[derive(Debug, Default)]
pub struct ProcessorClocks {
    /// Highest maximum across the logical processors.
    pub max_mhz: Option<f32>,
    /// Mean of the current clocks, as a sum and a count.
    pub current_sum: f32,
    /// How many processors reported a current clock.
    pub current_count: usize,
}

impl ProcessorClocks {
    /// Average current clock, when at least one processor reported one.
    pub fn current_mhz(&self) -> Option<f32> {
        if self.current_count == 0 {
            return None;
        }
        Some(self.current_sum / self.current_count as f32)
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
    fn records_are_walked_by_their_own_size_field() {
        // Two 16-byte records: `Relationship` at 0 is 0 for a processor core,
        // which a stride-0 bug once mistook for the end of the buffer.
        let mut bytes = vec![0u8; 32];
        bytes[4..8].copy_from_slice(&16u32.to_le_bytes());
        bytes[20..24].copy_from_slice(&16u32.to_le_bytes());
        assert_eq!(
            processor_records(&bytes),
            vec![(0, 16), (16, 16)],
            "a relationship of 0 must not end the walk"
        );

        // A record that claims more bytes than the buffer holds is not read.
        bytes[4..8].copy_from_slice(&64u32.to_le_bytes());
        assert_eq!(processor_records(&bytes), Vec::<(usize, usize)>::new());
    }

    #[test]
    fn the_efficiency_split_needs_two_classes() {
        assert_eq!(split_by_efficiency_classes(&[]), (None, None));
        assert_eq!(split_by_efficiency_classes(&[0, 0, 0]), (None, None));
        // Six performance cores and four efficiency ones on an Intel hybrid part.
        assert_eq!(
            split_by_efficiency_classes(&[0, 0, 0, 0, 0, 0, 2, 2, 2, 2]),
            (Some(6), Some(4))
        );
    }

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
        assert!(
            info.utc_offset_seconds.is_some(),
            "the local/UTC difference must be readable"
        );
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
