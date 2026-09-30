//! Process adapter built on `sysinfo`, shared by every platform.
//!
//! `sysinfo` is a Rust library that talks to the kernel directly (Level 1 of the
//! native-first policy: no shell, no `ps` parsing). Only termination is handled
//! natively per platform, because that is where error mapping and privilege
//! guidance matter.

use std::sync::Mutex;
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
#[cfg(test)]
use x_core::error::ErrorKind;
#[cfg(unix)]
use x_core::error::ErrorKind as _ErrorKind;
use x_core::error::{Error, PermissionRequirement, Result};
use x_core::process::{
    KillSignal, ProcessInfo, ProcessListOptions, ProcessManager, ProcessSort, ProcessState,
};

/// Minimum interval between two usage samples.
///
/// CPU utilization is a delta between two readings, so a first-ever snapshot
/// can only report zero. Waiting a short while before the second refresh turns
/// `x ps` into a real ranking instead of a list of zeroes, and costs nothing
/// when the frontend only wants a cheap snapshot.
const USAGE_SAMPLE_INTERVAL: Duration = Duration::from_millis(120);

/// `sysinfo` backed process manager.
#[derive(Debug, Default)]
pub struct SysinfoProcess {
    system: Mutex<System>,
    last_usage_sample: Mutex<Option<Instant>>,
}

impl SysinfoProcess {
    /// Create a manager with an empty cache; the first call primes it.
    pub fn new() -> Self {
        Self {
            system: Mutex::new(System::new()),
            last_usage_sample: Mutex::new(None),
        }
    }

    /// Create a manager and pay the first refresh upfront.
    pub fn preloaded() -> Self {
        let this = Self::new();
        if let Ok(mut system) = this.system.lock() {
            refresh(&mut system, true);
        }
        this
    }

    /// Refresh the cache and return a snapshot, sharing one refresh across all
    /// callers that ask within the same refresh cycle.
    fn snapshot(&self, options: &ProcessListOptions) -> Result<Vec<ProcessInfo>> {
        let mut system = self
            .system
            .lock()
            .map_err(|_| Error::system("process cache poisoned"))?;

        // A brand new `System` learns process names on its first refresh and
        // only fills in parent, owner, memory and state on the second one, so a
        // first-ever snapshot needs both passes or every column reads empty.
        if system.processes().is_empty() {
            refresh(&mut system, true);
        }

        if options.with_usage {
            self.wait_for_usage_window();
            refresh(&mut system, true);
            *self
                .last_usage_sample
                .lock()
                .map_err(|_| Error::system("process cache poisoned"))? = Some(Instant::now());
        } else {
            refresh(&mut system, false);
        }

        Ok(collect(&system, options))
    }

    /// Block until the previous usage sample is old enough to measure against.
    ///
    /// The lock is intentionally held while waiting: two concurrent callers must
    /// not both conclude that their own refresh is the second one.
    fn wait_for_usage_window(&self) {
        let Ok(last) = self.last_usage_sample.lock() else {
            return;
        };
        if let Some(previous) = *last {
            let elapsed = previous.elapsed();
            if elapsed < USAGE_SAMPLE_INTERVAL {
                std::thread::sleep(USAGE_SAMPLE_INTERVAL - elapsed);
            }
        }
    }

    /// Process name for a single pid, using a fresh lightweight pass.
    pub fn name_for(&self, pid: u32) -> Option<String> {
        let mut system = self.system.lock().ok()?;
        refresh(&mut system, false);
        system
            .process(Pid::from_u32(pid))
            .map(|p| p.name().to_string_lossy().into_owned())
    }

    /// Send a native termination request to a single pid.
    pub fn kill_native(&self, pid: u32, signal: KillSignal) -> Result<()> {
        let mut system = self
            .system
            .lock()
            .map_err(|_| Error::system("process cache poisoned"))?;
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[Pid::from_u32(pid)]),
            false,
            ProcessRefreshKind::nothing(),
        );
        let process = system
            .process(Pid::from_u32(pid))
            .ok_or_else(|| Error::not_found(format!("process {pid} not found")))?;

        let result = if signal == KillSignal::Kill {
            process.kill()
        } else {
            process
                .kill_with(to_sysinfo_signal(signal))
                .ok_or_else(|| Error::system("platform refused this signal"))?
        };

        if result {
            Ok(())
        } else {
            Err(permission_or_failure(pid))
        }
    }
}

fn refresh(system: &mut System, with_usage: bool) {
    let kind = if with_usage {
        ProcessRefreshKind::everything()
    } else {
        ProcessRefreshKind::nothing()
            .with_exe(UpdateKind::OnlyIfNotSet)
            .with_cmd(UpdateKind::OnlyIfNotSet)
    };
    system.refresh_processes_specifics(ProcessesToUpdate::All, true, kind);
}

#[cfg(unix)]
fn to_sysinfo_signal(signal: KillSignal) -> sysinfo::Signal {
    use sysinfo::Signal;
    match signal {
        KillSignal::Terminate => Signal::Term,
        KillSignal::Interrupt => Signal::Interrupt,
        KillSignal::Kill => Signal::Kill,
        KillSignal::Quit => Signal::Quit,
        KillSignal::Hangup => Signal::Hangup,
    }
}

#[cfg(windows)]
fn to_sysinfo_signal(_signal: KillSignal) -> sysinfo::Signal {
    // Windows has no signals; `sysinfo` maps every request to TerminateProcess.
    sysinfo::Signal::Term
}

/// Map a failed kill to the most useful error we can produce.
///
/// On Unix the errno tells us exactly why, so a permission failure becomes a
/// privilege message instead of a bare "not allowed".
#[cfg(unix)]
fn permission_or_failure(pid: u32) -> Error {
    let errno = std::io::Error::last_os_error();
    match errno.raw_os_error() {
        Some(libc::EPERM) | Some(libc::EACCES) => Error::permission_denied(
            PermissionRequirement::Elevated,
            format!("not allowed to signal process {pid}"),
        ),
        Some(libc::ESRCH) => Error::not_found(format!("process {pid} is already gone")),
        _ => Error::new(
            _ErrorKind::System,
            format!("cannot kill process {pid}: {errno}"),
        ),
    }
}

#[cfg(not(unix))]
fn permission_or_failure(pid: u32) -> Error {
    Error::permission_denied(
        PermissionRequirement::Administrator,
        format!("cannot open process {pid} for termination"),
    )
}

fn collect(system: &System, options: &ProcessListOptions) -> Vec<ProcessInfo> {
    let term = options
        .search
        .as_deref()
        .map(str::to_ascii_lowercase)
        .filter(|t| !t.is_empty());

    let mut rows: Vec<ProcessInfo> = system
        .processes()
        .iter()
        .filter_map(|(pid, process)| {
            let info = to_info(*pid, process, options.with_usage);
            if !matches_term(&info, term.as_deref()) {
                return None;
            }
            if let Some(user) = options.user.as_deref() {
                let matches = info
                    .user
                    .as_deref()
                    .is_some_and(|u| u.eq_ignore_ascii_case(user));
                if !matches {
                    return None;
                }
            }
            Some(info)
        })
        .collect();

    sort(&mut rows, options.sort);
    if let Some(limit) = options.limit {
        rows.truncate(limit);
    }
    rows
}

fn matches_term(info: &ProcessInfo, term: Option<&str>) -> bool {
    let Some(term) = term else {
        return true;
    };
    let name = info.name.to_ascii_lowercase();
    if name.contains(term) {
        return true;
    }
    info.command_line
        .as_deref()
        .is_some_and(|c| c.to_ascii_lowercase().contains(term))
        || info
            .executable
            .as_deref()
            .is_some_and(|e| e.to_ascii_lowercase().contains(term))
}

fn sort(rows: &mut [ProcessInfo], key: ProcessSort) {
    match key {
        ProcessSort::Cpu => rows.sort_by(|a, b| {
            b.cpu_usage
                .unwrap_or_default()
                .total_cmp(&a.cpu_usage.unwrap_or_default())
                .then_with(|| a.pid.cmp(&b.pid))
        }),
        ProcessSort::Memory => rows.sort_by(|a, b| {
            b.memory_bytes
                .unwrap_or_default()
                .cmp(&a.memory_bytes.unwrap_or_default())
                .then_with(|| a.pid.cmp(&b.pid))
        }),
        ProcessSort::Pid => rows.sort_by_key(|p| p.pid),
        ProcessSort::Name => rows.sort_by(|a, b| {
            a.name
                .to_ascii_lowercase()
                .cmp(&b.name.to_ascii_lowercase())
                .then_with(|| a.pid.cmp(&b.pid))
        }),
        ProcessSort::StartTime => rows.sort_by(|a, b| {
            a.start_time
                .cmp(&b.start_time)
                .then_with(|| a.pid.cmp(&b.pid))
        }),
    }
}

fn to_info(pid: Pid, process: &sysinfo::Process, with_usage: bool) -> ProcessInfo {
    ProcessInfo {
        pid: pid.as_u32(),
        parent_pid: process.parent().map(Pid::as_u32),
        name: process.name().to_string_lossy().into_owned(),
        executable: process.exe().map(|p| p.to_string_lossy().into_owned()),
        command_line: command_line(process),
        user: user_name(process),
        cpu_usage: with_usage.then(|| process.cpu_usage()),
        memory_bytes: Some(process.memory()),
        virtual_memory_bytes: Some(process.virtual_memory()),
        threads: process.tasks().map(|t| t.len() as u32),
        start_time: Some(process.start_time()),
        state: to_state(process.status()),
    }
}

fn command_line(process: &sysinfo::Process) -> Option<String> {
    let cmd = process.cmd();
    if cmd.is_empty() {
        return None;
    }
    Some(
        cmd.iter()
            .map(|part| part.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" "),
    )
}

fn user_name(process: &sysinfo::Process) -> Option<String> {
    let uid = process.user_id().or_else(|| process.effective_user_id())?;
    uid_to_name(uid)
}

#[cfg(unix)]
fn uid_to_name(uid: &sysinfo::Uid) -> Option<String> {
    use std::ffi::CStr;
    // SAFETY: `getpwuid` returns a pointer to a static buffer or NULL.
    unsafe {
        let passwd = libc::getpwuid(**uid);
        if passwd.is_null() {
            return None;
        }
        let name = (*passwd).pw_name;
        if name.is_null() {
            return None;
        }
        Some(CStr::from_ptr(name).to_string_lossy().into_owned())
    }
}

#[cfg(windows)]
fn uid_to_name(_uid: &sysinfo::Uid) -> Option<String> {
    // `sysinfo` reports no user id on Windows at all, so the owner name is
    // resolved from the process token by the platform adapter instead. This hook
    // exists only so the shared adapter has one place to reach for it.
    None
}

fn to_state(status: sysinfo::ProcessStatus) -> ProcessState {
    use sysinfo::ProcessStatus as S;
    match status {
        S::Run => ProcessState::Running,
        S::Sleep => ProcessState::Sleeping,
        S::Stop => ProcessState::Stopped,
        S::Zombie => ProcessState::Zombie,
        S::Idle => ProcessState::Idle,
        S::Dead => ProcessState::Terminated,
        _ => ProcessState::Unknown,
    }
}

impl ProcessManager for SysinfoProcess {
    fn list(&self, options: &ProcessListOptions) -> Result<Vec<ProcessInfo>> {
        self.snapshot(options)
    }

    fn kill(&self, pid: u32, signal: KillSignal) -> Result<()> {
        self.kill_native(pid, signal)
    }

    fn get(&self, pid: u32) -> Result<ProcessInfo> {
        self.snapshot(&ProcessListOptions::light())?
            .into_iter()
            .find(|p| p.pid == pid)
            .ok_or_else(|| Error::not_found(format!("process {pid} not found")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sees_this_test_process() {
        let manager = SysinfoProcess::new();
        let me = std::process::id();
        let info = manager.get(me).expect("own process");
        assert_eq!(info.pid, me);
        assert!(!info.name.is_empty());
        assert!(info.memory_bytes.is_some(), "memory must be reported");
        // A light refresh skips memory accounting; a full one must not.
        let full = manager
            .list(&ProcessListOptions {
                search: Some(info.name.clone()),
                with_usage: true,
                ..Default::default()
            })
            .expect("list");
        let full = full.iter().find(|p| p.pid == me).expect("self row");
        assert!(full.memory_bytes.unwrap_or(0) > 0);
        assert!(full.cpu_usage.is_some());
    }

    #[test]
    fn search_filters_by_name_or_command_line() {
        let manager = SysinfoProcess::new();
        let rows = manager
            .list(&ProcessListOptions::search("cargo"))
            .expect("list");
        // The test binary itself matches on its path, so this must not be empty
        // when run under cargo, and must never include unrelated shells.
        for row in &rows {
            let haystack = format!(
                "{} {}",
                row.name,
                row.command_line.as_deref().unwrap_or_default()
            )
            .to_ascii_lowercase();
            assert!(haystack.contains("cargo"), "unexpected row {row:?}");
        }
    }

    #[test]
    fn light_options_skip_cpu_accounting() {
        let manager = SysinfoProcess::new();
        let rows = manager.list(&ProcessListOptions::light()).expect("list");
        assert!(rows.iter().all(|r| r.cpu_usage.is_none()));
    }

    #[test]
    fn sorting_by_memory_puts_the_largest_first() {
        let manager = SysinfoProcess::new();
        let rows = manager
            .list(&ProcessListOptions {
                sort: ProcessSort::Memory,
                with_usage: false,
                ..Default::default()
            })
            .expect("list");
        if rows.len() >= 2 {
            assert!(rows[0].memory_bytes.unwrap_or(0) >= rows[1].memory_bytes.unwrap_or(0));
        }
    }

    #[test]
    fn limit_is_respected() {
        let manager = SysinfoProcess::new();
        let rows = manager
            .list(&ProcessListOptions {
                limit: Some(3),
                ..Default::default()
            })
            .expect("list");
        assert!(rows.len() <= 3);
    }

    #[test]
    fn unknown_pid_is_not_found() {
        let manager = SysinfoProcess::new();
        let err = manager.get(0x7fff_0000).expect_err("must fail");
        assert_eq!(err.kind(), ErrorKind::NotFound);
    }

    #[test]
    fn kills_a_process_it_started() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id();
        let manager = SysinfoProcess::new();
        manager.kill(pid, KillSignal::Kill).expect("kill");
        let status = child.wait().expect("wait");
        assert!(!status.success());
    }

    #[test]
    fn killing_a_dead_pid_reports_not_found() {
        let manager = SysinfoProcess::new();
        let child = std::process::Command::new("true").spawn().expect("spawn");
        let pid = child.id();
        child.wait_with_output().ok();
        std::thread::sleep(std::time::Duration::from_millis(200));
        let err = manager.kill(pid, KillSignal::Kill).expect_err("must fail");
        assert_eq!(err.kind(), ErrorKind::NotFound);
    }
}
