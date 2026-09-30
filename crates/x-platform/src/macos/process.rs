//! macOS process adapter.
//!
//! The shared `sysinfo` implementation already uses the Mach/`proc_listpids`
//! level APIs, so macOS only adds the details `sysinfo` does not expose: the
//! executable path via `proc_pidpath` and the owner user via `sysctl`.

use crate::common::process_sysinfo::SysinfoProcess;
use x_core::process::ProcessManager;

/// macOS process manager.
#[derive(Debug, Default)]
pub struct MacosProcess {
    inner: SysinfoProcess,
}

impl MacosProcess {
    /// Create the adapter.
    pub fn new() -> Self {
        Self {
            inner: SysinfoProcess::new(),
        }
    }
}

impl ProcessManager for MacosProcess {
    fn list(
        &self,
        options: &x_core::process::ProcessListOptions,
    ) -> x_core::error::Result<Vec<x_core::process::ProcessInfo>> {
        self.inner.list(options)
    }

    fn kill(&self, pid: u32, signal: x_core::process::KillSignal) -> x_core::error::Result<()> {
        self.inner.kill(pid, signal)
    }

    fn get(&self, pid: u32) -> x_core::error::Result<x_core::process::ProcessInfo> {
        self.inner.get(pid)
    }
}

/// Trait object helper.
pub fn manager() -> std::sync::Arc<dyn ProcessManager> {
    std::sync::Arc::new(MacosProcess::new())
}

/// Default adapter.
pub fn as_manager() -> std::sync::Arc<dyn ProcessManager> {
    manager()
}

/// Executable path for a pid, used by the TUI detail panel.
pub fn executable_of(pid: u32) -> Option<String> {
    crate::macos::libproc::pid_path(pid as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use x_core::process::ProcessListOptions;

    #[test]
    fn lists_processes_with_paths() {
        let rows = MacosProcess::new()
            .list(&ProcessListOptions::light())
            .expect("list");
        assert!(!rows.is_empty());
        let me = rows
            .iter()
            .find(|p| p.pid == std::process::id())
            .expect("self");
        assert!(!me.name.is_empty());
    }

    #[test]
    fn executable_path_is_resolvable() {
        let path = executable_of(std::process::id()).expect("own path");
        assert!(path.starts_with('/'));
    }
}
