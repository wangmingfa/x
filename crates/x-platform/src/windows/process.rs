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
        // back without a user are resolved.
        for row in &mut rows {
            if row.user.is_none() {
                row.user = super::account_name(row.pid);
            }
        }
        Ok(rows)
    }

    fn get(&self, pid: u32) -> Result<ProcessInfo> {
        let mut row = self.inner.get(pid)?;
        if row.user.is_none() {
            row.user = super::account_name(pid);
        }
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
