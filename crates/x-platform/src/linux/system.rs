//! Linux system adapter: `sysinfo` for CPU and memory, `/etc/os-release` for the
//! distribution name, `/proc/cpuinfo` for the physical core count.

use crate::common::{cpu_sysinfo, identity};
use crate::sys;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use x_core::error::Result;
use x_core::system::{
    CpuUsage, MemoryUsage, OsFamily, PressureLevel, SystemInfo, SystemManager,
};

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
        let clusters = cpufreq_clusters();
        let (performance_cores, efficiency_cores) =
            split_clusters(&clusters, &core_identity_by_cpu());

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
        // `cpufreq` is the authority on Linux for the ceiling and the policy;
        // the temperature comes from whichever sensor family knows the package.
        let clusters = cpufreq_clusters();
        usage.max_frequency_mhz = clusters
            .iter()
            .map(|cluster| cluster.max_khz)
            .max()
            .map(|khz| khz as f32 / 1_000.0);
        usage.temperature_celsius = cpu_temperature();
        usage.governor = governor(&clusters);
        Ok(usage)
    }

    fn memory_usage(&self) -> Result<MemoryUsage> {
        // `/proc/meminfo` is authoritative; sysinfo already parses it, including
        // the swap totals that `/proc/swaps` only shows for active devices.
        let info = sysinfo::System::new_all();
        let total = info.total_memory();
        let available = info.available_memory();
        Ok(MemoryUsage {
            total_bytes: total,
            used_bytes: total.saturating_sub(available),
            available_bytes: available,
            percent: percent(total.saturating_sub(available), total),
            swap_total_bytes: info.total_swap(),
            swap_used_bytes: info.used_swap(),
            pressure: memory_pressure(),
        })
    }
}

/// Kernel memory pressure from `/proc/pressure/memory` (PSI).
///
/// The file reports stall percentages for `some` (at least one task stalled)
/// and `full` (all non-idle tasks stalled) over 10s/60s/300s windows. Kernels
/// or configs without PSI (`CONFIG_PSI=n`, or the file hidden by a container)
/// yield `None` — a `/proc/pressure` reader must tolerate an empty read.
fn memory_pressure() -> Option<PressureLevel> {
    let raw = std::fs::read_to_string("/proc/pressure/memory").ok()?;
    memory_pressure_from(&raw)
}

/// Pure core of [`memory_pressure`], injectable for tests.
fn memory_pressure_from(raw: &str) -> Option<PressureLevel> {
    // `full` is the harder signal: tasks are stalling on memory right now.
    let full = raw.lines().find_map(|line| {
        let (name, rest) = line.split_once(' ')?;
        (name == "full").then(|| parse_psi_avg10(rest))
    })?;
    Some(match full {
        value if value >= 25.0 => PressureLevel::Critical,
        value if value >= 5.0 => PressureLevel::Warning,
        _ => PressureLevel::Normal,
    })
}

/// The `avg10=` value of a PSI line, e.g. `avg10=1.23 avg60=0.00 avg300=0.00 total=456`.
fn parse_psi_avg10(line: &str) -> f32 {
    line.split_whitespace()
        .find_map(|field| field.strip_prefix("avg10="))
        .and_then(|value| value.parse().ok())
        .unwrap_or(0.0)
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

/// Locale of the running process.
///
/// A login shell exports `LANG`; a systemd unit usually does not, and then the
/// value the administrator configured lives in `/etc/locale.conf` (systemd
/// convention) or `/etc/default/locale` (Debian family).
fn locale() -> Option<String> {
    identity::locale_name().or_else(configured_locale)
}

/// `LANG` or `LC_*` from the system locale file, if one exists.
fn configured_locale() -> Option<String> {
    ["/etc/locale.conf", "/etc/default/locale"]
        .into_iter()
        .find_map(|path| std::fs::read_to_string(path).ok())
        .and_then(|raw| parse_locale_file(&raw))
}

/// Pick the locale out of `KEY=value` lines, ignoring comments.
fn parse_locale_file(raw: &str) -> Option<String> {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#'))
        .filter_map(|line| line.split_once('='))
        .filter(|(key, _)| matches!(key.trim(), "LANG" | "LC_ALL" | "LC_CTYPE"))
        .map(|(_, value)| value.trim().trim_matches('"').to_string())
        .find(|value| !value.is_empty())
}

/// A frequency policy: one ceiling shared by a group of logical CPUs.
///
/// Kernels with `intel_pstate` or `cpufreq` expose one `policy<N>` directory per
/// cluster, which is exactly the granularity hybrid parts publish: P-cores and
/// E-cores never share a ceiling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CpufreqCluster {
    /// `cpuinfo_max_freq`, in kHz.
    pub max_khz: u64,
    /// `scaling_governor`, the policy the kernel uses to pick a frequency.
    pub governor: Option<String>,
    /// Logical CPUs following this policy, from `related_cpus`.
    pub cpus: Vec<u32>,
}

/// Every readable `cpufreq` policy directory, highest ceiling first.
pub fn cpufreq_clusters() -> Vec<CpufreqCluster> {
    let mut clusters: Vec<CpufreqCluster> = sysfs_directories("/sys/devices/system/cpu/cpufreq")
        .into_iter()
        .filter_map(|directory| {
            let max_khz = read_u64(&directory.join("cpuinfo_max_freq"))?;
            // `related_cpus` is the clock domain; a kernel that hides it leaves
            // the policy number as the only clue about which CPU it serves.
            let cpus = read_trimmed(&directory.join("related_cpus"))
                .map(|raw| parse_cpu_list(&raw))
                .unwrap_or_else(|| policy_number(&directory).into_iter().collect());
            Some(CpufreqCluster {
                max_khz,
                governor: read_trimmed(&directory.join("scaling_governor")),
                cpus,
            })
        })
        .collect();
    clusters.sort_by_key(|cluster| std::cmp::Reverse(cluster.max_khz));
    clusters
}

/// The CPU index a `policy<N>` directory is named after.
fn policy_number(directory: &Path) -> Option<u32> {
    let name = directory.file_name()?.to_string_lossy().into_owned();
    name.strip_prefix("policy")
        .and_then(|digits| digits.parse().ok())
}

/// Parse a sysfs CPU list: `0 1 2`, `0,1,2` and `0-3,8` all appear in the wild.
pub fn parse_cpu_list(raw: &str) -> Vec<u32> {
    let mut cpus = Vec::new();
    for item in raw.split([',', ' ', '\n']) {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        match item.split_once('-') {
            Some((start, end)) => {
                if let (Ok(start), Ok(end)) =
                    (start.trim().parse::<u32>(), end.trim().parse::<u32>())
                {
                    cpus.extend(start..=end);
                }
            }
            None => {
                if let Ok(cpu) = item.parse::<u32>() {
                    cpus.push(cpu);
                }
            }
        }
    }
    cpus
}

/// `(performance, efficiency)` physical cores, or `None` for a homogeneous part.
///
/// The cluster with the highest ceiling is the performance one. `topology` maps
/// a logical CPU to its `(package, core)` identity, which turns SMT siblings
/// into one core; a CPU missing from it counts as a core of its own, which is
/// right on machines without SMT and on kernels that hide the topology.
pub fn split_clusters(
    clusters: &[CpufreqCluster],
    topology: &HashMap<u32, (u32, u32)>,
) -> (Option<usize>, Option<usize>) {
    let mut ceilings: Vec<u64> = clusters.iter().map(|cluster| cluster.max_khz).collect();
    ceilings.sort_unstable();
    ceilings.dedup();
    if ceilings.len() < 2 {
        return (None, None);
    }
    let highest = *ceilings.last().unwrap_or(&0);

    let mut performance = 0;
    let mut efficiency = 0;
    let mut counted: HashSet<(u32, u32)> = HashSet::new();
    for cluster in clusters {
        let group = if cluster.max_khz == highest {
            &mut performance
        } else {
            &mut efficiency
        };
        for cpu in &cluster.cpus {
            // The out-of-range package keeps an unidentified CPU from colliding
            // with a real `(package, core)` pair.
            let identity = topology.get(cpu).copied().unwrap_or((u32::MAX, *cpu));
            if counted.insert(identity) {
                *group += 1;
            }
        }
    }
    (
        (performance > 0).then_some(performance),
        (efficiency > 0).then_some(efficiency),
    )
}

/// `(physical id, core id)` per logical CPU from `/proc/cpuinfo`.
pub fn core_identity_by_cpu() -> HashMap<u32, (u32, u32)> {
    parse_cpuinfo_cores(&std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default())
}

/// Parse the per-processor `physical id` / `core id` pairs of `/proc/cpuinfo`.
///
/// Sections arrive as `processor : N` blocks; a kernel that publishes neither
/// field (ARM, most VMs) yields an empty map.
pub fn parse_cpuinfo_cores(raw: &str) -> HashMap<u32, (u32, u32)> {
    let mut identities = HashMap::new();
    let mut processor: Option<u32> = None;
    let mut package: Option<u32> = None;
    let mut core: Option<u32> = None;
    for line in raw.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "processor" => {
                processor = value.parse().ok();
                package = None;
                core = None;
            }
            "physical id" => package = value.parse().ok(),
            "core id" => core = value.parse().ok(),
            _ => continue,
        }
        if let (Some(processor), Some(package), Some(core)) = (processor, package, core) {
            identities.insert(processor, (package, core));
        }
    }
    identities
}

/// The scaling governor, joined when the clusters disagree.
pub fn governor(clusters: &[CpufreqCluster]) -> Option<String> {
    let mut names: Vec<String> = Vec::new();
    for cluster in clusters {
        if let Some(name) = &cluster.governor {
            if !name.is_empty() && !names.contains(name) {
                names.push(name.clone());
            }
        }
    }
    (!names.is_empty()).then(|| names.join("/"))
}

/// Hottest CPU sensor reading, in degrees Celsius.
///
/// Both families of nodes are read: `thermal_zone*` describes the package on
/// x86 and ARM, and `hwmon` carries the per-core sensors of `coretemp`/`k10temp`.
/// Sensors with no CPU in the name are skipped, because a board thermistor is
/// not the die temperature the user asked for.
pub fn cpu_temperature() -> Option<f32> {
    let mut hottest: Option<f32> = None;
    for zone in sysfs_directories("/sys/class/thermal") {
        let Some(name) = read_trimmed(&zone.join("type")) else {
            continue;
        };
        if !is_cpu_sensor(&name) {
            continue;
        }
        let reading = read_trimmed(&zone.join("temp"));
        let Some(raw) = reading.as_deref() else {
            continue;
        };
        if let Some(temp) = parse_milli_celsius(raw) {
            hottest = Some(hottest.map_or(temp, |current| current.max(temp)));
        }
    }
    for chip in sysfs_directories("/sys/class/hwmon") {
        let Some(name) = read_trimmed(&chip.join("name")) else {
            continue;
        };
        if !is_cpu_sensor(&name) {
            continue;
        }
        for input in sysfs_files_named(&chip, "temp", "_input") {
            if let Some(temp) = read_u64(&input).and_then(|milli| milli_to_celsius(milli as i64)) {
                hottest = Some(hottest.map_or(temp, |current| current.max(temp)));
            }
        }
    }
    hottest
}

/// Is this sensor name about the CPU package?
pub fn is_cpu_sensor(name: &str) -> bool {
    let name = name.to_lowercase();
    matches!(
        name.as_str(),
        "coretemp" | "k10temp" | "zenpower" | "cpu_thermal" | "soc_thermal"
    ) || name.contains("cpu")
        || name.contains("pkg")
}

/// A sysfs milli-degree reading into Celsius, skipping unset sensors.
///
/// Drivers report `not read yet` as `0` and out-of-range as a saturated value,
/// so a plausible window is the only validation x can do.
pub fn parse_milli_celsius(raw: &str) -> Option<f32> {
    raw.trim().parse::<i64>().ok().and_then(milli_to_celsius)
}

/// Milli-degrees into Celsius, with the plausibility window above.
pub fn milli_to_celsius(milli: i64) -> Option<f32> {
    let celsius = milli as f64 / 1000.0;
    (celsius > 0.0 && celsius < 150.0).then_some(celsius as f32)
}

/// Directories below `path`, sorted so readings are stable across runs.
fn sysfs_directories(path: &str) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = children(Path::new(path))
        .into_iter()
        .filter(|path| path.is_dir())
        .collect();
    entries.sort();
    entries
}

/// Files of `parent` named `prefix<N>suffix`, the hwmon reading convention.
fn sysfs_files_named(parent: &Path, prefix: &str, suffix: &str) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = children(parent)
        .into_iter()
        .filter(|path| {
            path.file_name().is_some_and(|name| {
                let name = name.to_string_lossy();
                name.starts_with(prefix) && name.ends_with(suffix)
            })
        })
        .collect();
    entries.sort();
    entries
}

/// Entries of a sysfs directory, empty when the directory does not exist.
fn children(path: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(path)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .collect()
}

/// A trimmed sysfs string, when the file exists and has content.
fn read_trimmed(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let trimmed = raw.trim().to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

fn read_u64(path: &Path) -> Option<u64> {
    read_trimmed(path)?.parse().ok()
}

/// Trait object helper.
pub fn manager() -> std::sync::Arc<dyn SystemManager> {
    std::sync::Arc::new(LinuxSystem::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pressure_reads_the_full_line() {
        let raw = "some avg10=0.00 avg60=0.00 avg300=0.00 total=0\n\
                   full avg10=3.14 avg60=0.00 avg300=0.00 total=0\n";
        assert_eq!(memory_pressure_from(raw), Some(PressureLevel::Warning));
        assert_eq!(
            memory_pressure_from("some avg10=0.00 avg60=0.00 avg300=0.00 total=0"),
            Some(PressureLevel::Normal)
        );
        assert_eq!(memory_pressure_from(""), None);
    }

    #[test]
    fn psi_avg10_parses_or_defaults_to_zero() {
        assert_eq!(parse_psi_avg10("avg10=12.5 avg60=0.00"), 12.5);
        assert_eq!(parse_psi_avg10("avg60=0.00"), 0.0);
        assert_eq!(parse_psi_avg10("avg10=garbage"), 0.0);
    }

    /// A cluster with no governor, so tests set only what they assert on.
    fn cluster(max_khz: u64, cpus: &[u32]) -> CpufreqCluster {
        CpufreqCluster {
            max_khz,
            governor: None,
            cpus: cpus.to_vec(),
        }
    }

    #[test]
    fn cpu_lists_accept_every_sysfs_shape() {
        assert_eq!(parse_cpu_list("0 1 2 3"), vec![0, 1, 2, 3]);
        assert_eq!(parse_cpu_list("0,1,8"), vec![0, 1, 8]);
        assert_eq!(parse_cpu_list("0-3,8\n"), vec![0, 1, 2, 3, 8]);
        assert_eq!(parse_cpu_list("not a cpu"), Vec::<u32>::new());
    }

    #[test]
    fn a_hybrid_split_counts_physical_cores_not_threads() {
        // Six performance cores with SMT, four efficiency cores without.
        let performance = cluster(
            5_100_000,
            &[0, 1, 2, 3, 4, 5, 12, 13, 14, 15, 16, 17, 18, 19],
        );
        let efficiency = cluster(3_600_000, &[6, 7, 8, 9]);
        let topology = [
            (0, (0, 0)),
            (1, (0, 1)),
            (2, (0, 2)),
            (3, (0, 3)),
            (4, (0, 4)),
            (5, (0, 5)),
            (12, (0, 0)),
            (13, (0, 1)),
            (14, (0, 2)),
            (15, (0, 3)),
            (16, (0, 4)),
            (17, (0, 5)),
            (18, (0, 4)),
            (19, (0, 5)),
            (6, (0, 6)),
            (7, (0, 7)),
            (8, (0, 8)),
            (9, (0, 9)),
        ]
        .into_iter()
        .collect();

        assert_eq!(
            split_clusters(&[performance, efficiency], &topology),
            (Some(6), Some(4)),
            "SMT siblings must collapse into one core"
        );
    }

    #[test]
    fn a_homogeneous_part_has_no_split_to_make() {
        let clusters = [cluster(4_000_000, &[0, 1]), cluster(4_000_000, &[2, 3])];
        assert_eq!(split_clusters(&clusters, &HashMap::new()), (None, None));
    }

    #[test]
    fn cpus_without_topology_count_as_cores_of_their_own() {
        let clusters = [cluster(5_000_000, &[0, 1]), cluster(2_000_000, &[2])];
        assert_eq!(
            split_clusters(&clusters, &HashMap::new()),
            (Some(2), Some(1))
        );
    }

    #[test]
    fn governors_are_deduplicated_in_cluster_order() {
        let mut fast = cluster(5_000_000, &[0]);
        fast.governor = Some("performance".into());
        let mut slow = cluster(2_000_000, &[4]);
        slow.governor = Some("powersave".into());
        assert_eq!(
            governor(&[fast.clone(), slow.clone()]).as_deref(),
            Some("performance/powersave")
        );
        assert_eq!(
            governor(&[fast.clone(), fast]).as_deref(),
            Some("performance")
        );
        assert_eq!(governor(&[cluster(1, &[0]), slow]).as_deref(), None);
    }

    #[test]
    fn cpuinfo_cores_are_pairwise_per_processor() {
        let raw = "processor\t: 0\nphysical id\t: 0\ncore id\t\t: 0\n\
                   processor\t: 1\nphysical id\t: 0\ncore id\t\t: 0\n\
                   processor\t: 2\nphysical id\t: 0\ncore id\t\t: 1\n";
        let identities = parse_cpuinfo_cores(raw);
        assert_eq!(identities.get(&0), Some(&(0, 0)));
        assert_eq!(identities.get(&1), Some(&(0, 0)), "the SMT sibling");
        assert_eq!(identities.get(&2), Some(&(0, 1)));
        assert!(
            parse_cpuinfo_cores("processor : 0\n").is_empty(),
            "no topology"
        );
    }

    #[test]
    fn temperatures_are_screened_for_plausible_sensors() {
        assert_eq!(parse_milli_celsius("52000"), Some(52.0));
        assert_eq!(parse_milli_celsius(" 45000 \n"), Some(45.0));
        assert_eq!(parse_milli_celsius("0"), None, "not read yet");
        assert_eq!(parse_milli_celsius("-40000"), None);
        assert_eq!(parse_milli_celsius("200000"), None, "saturated");
        assert!(is_cpu_sensor("x86_pkg_temp"));
        assert!(is_cpu_sensor("Coretemp"));
        assert!(is_cpu_sensor("cpu0_hot"));
        assert!(
            !is_cpu_sensor("acpitz"),
            "a board thermistor is not the die"
        );
        assert!(!is_cpu_sensor("bat_hot"));
    }

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
    fn locale_file_parsing_skips_comments_and_other_keys() {
        let raw = "# set by ansible\nLC_MEASUREMENT=en_US.UTF-8\nLANG=\"zh_CN.UTF-8\"\n";
        assert_eq!(parse_locale_file(raw).as_deref(), Some("zh_CN.UTF-8"));
        assert_eq!(parse_locale_file("# nothing here\n"), None);
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
        // A container may have no time zone file, but the clock offset always
        // comes from libc.
        assert!(info.utc_offset_seconds.is_some());
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
        assert!(cpu.load_average.is_some(), "/proc/loadavg is always there");
        if let Some(max) = cpu.max_frequency_mhz {
            assert!(max > 0.0, "a cpufreq ceiling must be positive");
        }
    }
}
