//! Linux process adapter.
//!
//! `sysinfo` reads `/proc` for every platform it supports, so the shared adapter
//! does the work; what Linux adds on top is the parent pid from
//! `/proc/<pid>/stat`, which is what the process tree is built from.

use crate::common::process_sysinfo::SysinfoProcess;
use std::collections::HashMap;
use x_core::error::Result;
use x_core::process::{ProcessInfo, ProcessListOptions, ProcessManager};

/// Reads Linux processes.
#[derive(Debug, Default)]
pub struct LinuxProcess {
    inner: SysinfoProcess,
}

impl LinuxProcess {
    /// Create the adapter.
    pub fn new() -> Self {
        Self {
            inner: SysinfoProcess::new(),
        }
    }
}

/// Parent pids from `/proc/<pid>/stat`, keyed by pid.
pub fn parents() -> HashMap<u32, u32> {
    let mut parents = HashMap::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return parents;
    };

    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else {
            continue;
        };
        // The comm field is parenthesised and may contain spaces, so the parent
        // pid is the field right after the last ')'.
        if let Some(rest) = stat.rsplit_once(')').map(|(_, rest)| rest) {
            if let Some(parent) = rest.split_whitespace().nth(1).and_then(|f| f.parse().ok()) {
                parents.insert(pid, parent);
            }
        }
    }
    parents
}

impl ProcessManager for LinuxProcess {
    fn list(&self, options: &ProcessListOptions) -> Result<Vec<ProcessInfo>> {
        let mut rows = self.inner.list(options)?;
        let parents = parents();
        for row in &mut rows {
            if row.parent_pid.is_none() {
                row.parent_pid = parents.get(&row.pid).copied();
            }
        }
        Ok(rows)
    }

    fn get(&self, pid: u32) -> Result<ProcessInfo> {
        let mut row = self.inner.get(pid)?;
        if row.parent_pid.is_none() {
            row.parent_pid = parents().get(&pid).copied();
        }
        Ok(row)
    }

    fn kill(&self, pid: u32, signal: x_core::process::KillSignal) -> Result<()> {
        self.inner.kill(pid, signal)
    }
}

/// Trait object helper.
pub fn manager() -> std::sync::Arc<dyn ProcessManager> {
    std::sync::Arc::new(LinuxProcess::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_pids_include_this_process_and_init() {
        let parents = parents();
        let me = std::process::id();
        assert!(parents.len() > 1, "no process tree read from /proc");
        assert!(parents.contains_key(&1), "init must be in /proc");
        if let Some(parent) = parents.get(&me) {
            assert!(*parent > 0);
        }
    }

    #[test]
    fn rows_carry_a_parent_pid() {
        let rows = LinuxProcess::new()
            .list(&ProcessListOptions::light())
            .expect("list");
        let me = std::process::id();
        let own = rows.iter().find(|r| r.pid == me).expect("self row");
        assert!(own.parent_pid.is_some(), "parent must be resolved on Linux");
        assert!(own.user.is_some(), "user must be resolved on Linux");
    }
}
