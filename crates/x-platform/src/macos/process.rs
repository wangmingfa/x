//! macOS process adapter.
//!
//! The shared `sysinfo` implementation already uses the Mach/`proc_listpids`
//! level APIs, so macOS only adds the details `sysinfo` does not expose: the
//! executable path via `proc_pidpath` and the owner user via `sysctl`.

use crate::common::process_sysinfo::SysinfoProcess;
use x_core::process::{ProcessConnection, ProcessManager};

/// Open files of `pid`, paths only, capped for output sanity.
///
/// Only vnode descriptors carry a path; sockets and pipes show up as
/// connections or are skipped. Unreadable descriptors (other user's files)
/// are simply absent, the same permission boundary `lsof` draws.
fn open_files(pid: u32) -> Vec<String> {
    let pid = pid as i32;
    crate::macos::libproc::list_fds(pid)
        .into_iter()
        .filter(|fd| fd.proc_fdtype == crate::macos::libproc::PROX_FDTYPE_VNODE)
        .filter_map(|fd| crate::macos::libproc::fd_path(pid, fd.proc_fd))
        .take(256)
        .collect()
}

/// TCP/UDP connections of `pid`, from the per-descriptor socket info.
fn connections(pid: u32) -> Vec<ProcessConnection> {
    use crate::macos::libproc::{self, SOCKINFO_IN, SOCKINFO_TCP};

    fn address(raw: [u8; 16], port: u16) -> String {
        use std::net::{Ipv4Addr, Ipv6Addr};
        if raw[..12].iter().all(|b| *b == 0) {
            format!(
                "{}:{port}",
                Ipv4Addr::new(raw[12], raw[13], raw[14], raw[15])
            )
        } else {
            format!("{}:{port}", Ipv6Addr::from(raw))
        }
    }

    let pid = pid as i32;
    let mut rows = Vec::new();
    for fd in libproc::list_fds(pid) {
        if fd.proc_fdtype != libproc::PROX_FDTYPE_SOCKET {
            continue;
        }
        let Some(details) = libproc::socket_info(pid, fd.proc_fd) else {
            continue;
        };
        let protocol = match details.kind {
            SOCKINFO_TCP => "tcp",
            SOCKINFO_IN => "udp",
            _ => continue,
        };
        // The remote port is already byte swapped; a zero peer means listener.
        rows.push(ProcessConnection {
            protocol: protocol.into(),
            local: address(details.local_address, details.local_port),
            remote: if details.remote_port == 0 {
                String::new()
            } else {
                address(details.remote_address, details.remote_port)
            },
        });
    }
    rows
}

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
        let mut rows = self.inner.list(options)?;
        // `PROC_PIDLISTFDS` is one cheap call per pid — no path resolution —
        // so every list row can carry the descriptor count.
        for row in &mut rows {
            row.fd_count = Some(fd_count(row.pid));
        }
        Ok(rows)
    }

    fn kill(&self, pid: u32, signal: x_core::process::KillSignal) -> x_core::error::Result<()> {
        self.inner.kill(pid, signal)
    }

    fn get(&self, pid: u32) -> x_core::error::Result<x_core::process::ProcessInfo> {
        let mut info = self.inner.get(pid)?;
        info.fd_count = Some(fd_count(pid));
        info.open_files = Some(open_files(pid));
        info.connections = Some(connections(pid));
        Ok(info)
    }
}

/// Descriptor count of `pid`: the raw `PROC_PIDLISTFDS` entries, sockets and
/// pipes included. Unreadable pids (other users, dying processes) read as 0,
/// which is indistinguishable from "no descriptors" — the same boundary
/// `lsof` draws, reported as a count rather than hidden.
fn fd_count(pid: u32) -> u32 {
    crate::macos::libproc::list_fds(pid as i32).len() as u32
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
