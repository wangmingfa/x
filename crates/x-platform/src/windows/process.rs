//! Windows process adapter.
//!
//! `sysinfo` walks the toolhelp snapshot on Windows, so the shared adapter does
//! the listing; what only Windows has is the account name, which lives behind
//! the process token.

use crate::common::process_sysinfo::SysinfoProcess;
use x_core::error::Result;
use x_core::process::{ProcessInfo, ProcessListOptions, ProcessManager};

/// Reads Windows processes.
#[derive(Debug, Default)]
pub struct WindowsProcess {
    inner: SysinfoProcess,
}

impl WindowsProcess {
    /// Create the adapter.
    pub fn new() -> Self {
        Self {
            inner: SysinfoProcess::new(),
        }
    }
}

impl ProcessManager for WindowsProcess {
    fn list(&self, options: &ProcessListOptions) -> Result<Vec<ProcessInfo>> {
        let mut rows = self.inner.list(options)?;
        // Token lookups are comparatively expensive, so only the rows that came
        // back without a user are resolved. Handle counts are one cheap query.
        for row in &mut rows {
            if row.user.is_none() {
                row.user = super::account_name(row.pid);
            }
            row.fd_count = handle_count(row.pid);
        }
        Ok(rows)
    }

    fn get(&self, pid: u32) -> Result<ProcessInfo> {
        let mut row = self.inner.get(pid)?;
        if row.user.is_none() {
            row.user = super::account_name(pid);
        }
        row.fd_count = handle_count(pid);
        // Open files and per-process connections need driver-level or elevated
        // APIs on Windows (NtQuerySystemInformation / Restart Manager); until
        // those are wired, report an empty set instead of pretending the
        // process has none of either.
        row.open_files = Some(Vec::new());
        row.connections = Some(Vec::new());
        Ok(row)
    }

    fn kill(&self, pid: u32, signal: x_core::process::KillSignal) -> Result<()> {
        self.inner.kill(pid, signal)
    }
}

/// Handle count of `pid`, `None` when the process cannot be opened (it died,
/// or it is a protected system process) — a missing read is not a zero.
fn handle_count(pid: u32) -> Option<u32> {
    use windows_sys::Win32::Foundation::CloseHandle;
    // GetProcessHandleCount lives in Threading, not ProcessStatus.
    use windows_sys::Win32::System::Threading::GetProcessHandleCount;

    // SAFETY: `OpenProcess` returns a valid handle or null; the count pointer
    // is a plain u32 out-parameter for the duration of the call.
    unsafe {
        let process = windows_sys::Win32::System::Threading::OpenProcess(
            windows_sys::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        );
        if process.is_null() {
            return None;
        }
        let mut count: u32 = 0;
        let ok = GetProcessHandleCount(process, &mut count) != 0;
        CloseHandle(process);
        ok.then_some(count)
    }
}

/// Trait object helper.
pub fn manager() -> std::sync::Arc<dyn ProcessManager> {
    std::sync::Arc::new(WindowsProcess::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_current_process_has_an_account_name() {
        let info = WindowsProcess::new()
            .get(std::process::id())
            .expect("own process");
        assert!(!info.name.is_empty());
        assert!(
            info.user.as_deref().is_some_and(|user| !user.is_empty()),
            "token lookup must resolve the account"
        );
    }

    #[test]
    fn a_dead_pid_reports_not_found() {
        let err = WindowsProcess::new()
            .get(0xFFFF_FFFE)
            .expect_err("must not exist");
        assert_eq!(err.kind(), x_core::error::ErrorKind::NotFound);
    }
}
