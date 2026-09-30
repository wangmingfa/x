//! Process capability: what x can do with processes on every platform.

use crate::error::Result;
use crate::process::model::{KillSignal, ProcessInfo, ProcessListOptions, ProcessTree};

/// Platform independent process operations.
///
/// Only *snapshot* style methods live here on purpose: watching is built on
/// top of polling so the CLI (`x process list`) and the TUI (live refresh)
/// share exactly the same code path.
pub trait ProcessManager: Send + Sync {
    /// List processes matching `options`.
    fn list(&self, options: &ProcessListOptions) -> Result<Vec<ProcessInfo>>;

    /// Fetch a single process by pid.
    fn get(&self, pid: u32) -> Result<ProcessInfo> {
        self.list(&ProcessListOptions {
            limit: None,
            ..Default::default()
        })?
        .into_iter()
        .find(|p| p.pid == pid)
        .ok_or_else(|| crate::error::Error::not_found(format!("process {pid} not found")))
    }

    /// Find processes whose name, executable or command line matches `query`.
    fn find(&self, query: &str) -> Result<Vec<ProcessInfo>> {
        self.list(&ProcessListOptions::search(query))
    }

    /// Build the parent/child tree for processes matching `options`.
    fn tree(&self, options: &ProcessListOptions) -> Result<ProcessTree> {
        Ok(crate::process::model::build_tree(self.list(options)?))
    }

    /// Terminate a process with a semantic signal.
    fn kill(&self, pid: u32, signal: KillSignal) -> Result<()>;

    /// Terminate several processes, returning the pids that could not be killed.
    ///
    /// Partial failure is the normal case when killing a filtered list, so the
    /// errors are collected instead of short-circuiting.
    fn kill_many(&self, pids: &[u32], signal: KillSignal) -> Result<Vec<crate::error::Error>> {
        let mut failures = Vec::new();
        for &pid in pids {
            if let Err(e) = self.kill(pid, signal) {
                failures.push(e);
            }
        }
        Ok(failures)
    }
}
