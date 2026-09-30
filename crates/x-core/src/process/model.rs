//! Unified process model.

use serde::{Deserialize, Serialize};

/// Lifecycle state of a process, normalized across platforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessState {
    /// Running / running on a multi-core host.
    Running,
    /// Alive but blocked waiting on something.
    Sleeping,
    /// Stopped by a signal.
    Stopped,
    /// Process exists in the table but cannot be inspected.
    Zombie,
    /// Created but not scheduled yet.
    Idle,
    /// Terminating.
    Terminated,
    /// The platform does not expose this information.
    Unknown,
}

impl ProcessState {
    /// Compact label for tables and gauges.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Sleeping => "sleeping",
            Self::Stopped => "stopped",
            Self::Zombie => "zombie",
            Self::Idle => "idle",
            Self::Terminated => "terminated",
            Self::Unknown => "unknown",
        }
    }
}

/// Termination request, expressed semantically instead of as a raw signal number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KillSignal {
    /// Graceful termination (`SIGTERM` / `TerminateProcess`).
    Terminate,
    /// Interrupt (`SIGINT`, Windows console `CTRL_BREAK`).
    Interrupt,
    /// Hard kill (`SIGKILL`).
    Kill,
    /// Quit (`SIGQUIT`, dumps core on Unix).
    Quit,
    /// Hang up (`SIGHUP`).
    Hangup,
}

impl KillSignal {
    /// `true` when the signal cannot be caught or ignored by the target.
    pub const fn is_forceful(self) -> bool {
        matches!(self, Self::Kill)
    }

    /// Parse from the CLI friendly name.
    pub fn parse(value: &str) -> Option<Self> {
        match value.to_ascii_lowercase().as_str() {
            "term" | "terminate" | "15" => Some(Self::Terminate),
            "int" | "interrupt" | "2" => Some(Self::Interrupt),
            "kill" | "9" | "force" => Some(Self::Kill),
            "quit" | "3" => Some(Self::Quit),
            "hup" | "hangup" | "1" => Some(Self::Hangup),
            _ => None,
        }
    }

    /// Every accepted CLI value, used for error messages and shell completion.
    pub const ALL: &'static [(&'static str, Self)] = &[
        ("term", Self::Terminate),
        ("int", Self::Interrupt),
        ("kill", Self::Kill),
        ("quit", Self::Quit),
        ("hup", Self::Hangup),
    ];
}

impl std::fmt::Display for KillSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::Terminate => "term",
            Self::Interrupt => "int",
            Self::Kill => "kill",
            Self::Quit => "quit",
            Self::Hangup => "hup",
        };
        f.write_str(name)
    }
}

/// A process as observed on any supported platform.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessInfo {
    /// OS process id.
    pub pid: u32,
    /// Parent process id when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_pid: Option<u32>,
    /// Process name (executable name without extension on Windows).
    pub name: String,
    /// Full path of the executable when the platform exposes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    /// Full command line when the platform exposes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_line: Option<String>,
    /// Owning user name when the platform exposes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// CPU usage percentage since the previous sample.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_usage: Option<f32>,
    /// Resident memory in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_bytes: Option<u64>,
    /// Virtual address space in bytes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub virtual_memory_bytes: Option<u64>,
    /// Number of OS threads.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threads: Option<u32>,
    /// Unix boot-relative start time in seconds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_time: Option<u64>,
    /// Lifecycle state.
    pub state: ProcessState,
    /// Working directory when the platform exposes it. Populated only for
    /// detail views: walking `/proc`-style CWDs for every process is expensive.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Open file paths when the platform exposes them. Populated only for
    /// detail views and capped, a chatty process can hold thousands.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_files: Option<Vec<String>>,
    /// Network connections held by this process.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connections: Option<Vec<ProcessConnection>>,
    /// Environment variables when the platform exposes them. Populated only
    /// for detail views.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environment: Option<std::collections::BTreeMap<String, String>>,
}

/// A network connection owned by a process, in source/destination form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessConnection {
    /// Protocol name, e.g. `tcp` or `udp`.
    pub protocol: String,
    /// Local address as `ip:port`.
    pub local: String,
    /// Remote address as `ip:port`, empty for a listener.
    pub remote: String,
}

impl ProcessInfo {
    /// Best available human label for the process.
    pub fn label(&self) -> &str {
        &self.name
    }
}

/// Sort keys supported by [`ProcessListOptions`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessSort {
    /// Highest CPU usage first. Default.
    #[default]
    Cpu,
    /// Highest resident memory first.
    Memory,
    /// Process id.
    Pid,
    /// Process name, then pid.
    Name,
    /// Oldest process first.
    StartTime,
}

/// Filter and ordering options for process snapshots.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProcessListOptions {
    /// Case-insensitive substring matched against name, command line and executable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
    /// Only processes owned by this user.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Sort key.
    pub sort: ProcessSort,
    /// Maximum number of processes to return.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    /// Include CPU and memory accounting. Disabling it makes snapshots much cheaper.
    pub with_usage: bool,
}

impl ProcessListOptions {
    /// Options with the given search term, CPU sorted, usage enabled.
    pub fn search(term: impl Into<String>) -> Self {
        Self {
            search: Some(term.into()),
            sort: ProcessSort::Cpu,
            limit: None,
            user: None,
            with_usage: true,
        }
    }

    /// Cheap snapshot options (no CPU/memory accounting).
    pub fn light() -> Self {
        Self {
            search: None,
            user: None,
            sort: ProcessSort::Pid,
            limit: None,
            with_usage: false,
        }
    }
}

/// A process plus its children, produced by [`crate::process::ProcessManager::tree`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessNode {
    /// The process itself.
    pub process: ProcessInfo,
    /// Direct children, recursively.
    pub children: Vec<ProcessNode>,
}

impl ProcessNode {
    /// Depth of the subtree, used for indentation.
    pub fn depth_total(&self) -> usize {
        1 + self.children.iter().map(Self::depth_total).sum::<usize>()
    }
}

/// Flatten a tree into `(depth, process)` pairs for table output.
pub fn flatten_tree(node: &ProcessNode) -> Vec<(usize, &ProcessInfo)> {
    fn walk<'a>(node: &'a ProcessNode, depth: usize, out: &mut Vec<(usize, &'a ProcessInfo)>) {
        out.push((depth, &node.process));
        for child in &node.children {
            walk(child, depth + 1, out);
        }
    }
    let mut out = Vec::new();
    walk(node, 0, &mut out);
    out
}

/// Build a process tree from a flat snapshot. Orphans are attached to a synthetic root.
pub fn build_tree(processes: Vec<ProcessInfo>) -> ProcessTree {
    use std::collections::HashMap;

    let mut nodes: HashMap<u32, ProcessNode> = processes
        .into_iter()
        .map(|p| {
            (
                p.pid,
                ProcessNode {
                    process: p,
                    children: Vec::new(),
                },
            )
        })
        .collect();

    let roots_pids: Vec<u32> = nodes
        .values()
        .filter(|n| {
            n.process
                .parent_pid
                .is_some_and(|ppid| ppid != n.process.pid && nodes.contains_key(&ppid))
        })
        .map(|n| n.process.pid)
        .collect();

    let orphans: Vec<u32> = nodes
        .keys()
        .copied()
        .filter(|pid| !roots_pids.contains(pid))
        .collect();
    let mut sorted_orphans = orphans;
    sorted_orphans.sort_unstable();

    let mut roots = Vec::new();
    // Attach in reverse so that appending children preserves ascending pid order.
    for pid in sorted_orphans.into_iter().rev() {
        if let Some(mut node) = nodes.remove(&pid) {
            if let Some(children) = drain_children(&mut nodes, pid) {
                node.children = children;
            }
            roots.push(node);
        }
    }
    roots.sort_by_key(|n| n.process.pid);

    ProcessTree { roots }
}

fn drain_children(
    nodes: &mut std::collections::HashMap<u32, ProcessNode>,
    parent: u32,
) -> Option<Vec<ProcessNode>> {
    let child_pids: Vec<u32> = nodes
        .iter()
        .filter(|(_, n)| n.process.parent_pid == Some(parent))
        .map(|(pid, _)| *pid)
        .collect();
    if child_pids.is_empty() {
        return Some(Vec::new());
    }
    let mut children = Vec::with_capacity(child_pids.len());
    for pid in child_pids {
        if let Some(mut node) = nodes.remove(&pid) {
            if let Some(grand) = drain_children(nodes, pid) {
                node.children = grand;
            }
            children.push(node);
        }
    }
    children.sort_by_key(|n| n.process.pid);
    Some(children)
}

/// Result of [`crate::process::ProcessManager::tree`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProcessTree {
    /// Top level processes.
    pub roots: Vec<ProcessNode>,
}

impl ProcessTree {
    /// Number of processes in the tree.
    pub fn len(&self) -> usize {
        fn count(nodes: &[ProcessNode]) -> usize {
            nodes.iter().map(|n| 1 + count(&n.children)).sum()
        }
        count(&self.roots)
    }

    /// `true` when no process was found.
    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    /// Iterate depth-first with depth markers, skipping the synthetic root.
    pub fn iter(&self) -> impl Iterator<Item = (usize, &ProcessInfo)> {
        self.roots
            .iter()
            .flat_map(|n| crate::process::flatten_tree(n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, ppid: Option<u32>, name: &str) -> ProcessInfo {
        ProcessInfo {
            pid,
            parent_pid: ppid,
            name: name.to_string(),
            executable: None,
            command_line: None,
            user: None,
            cpu_usage: None,
            memory_bytes: None,
            virtual_memory_bytes: None,
            threads: None,
            start_time: None,
            state: ProcessState::Running,
            cwd: None,
            open_files: None,
            connections: None,
            environment: None,
        }
    }

    #[test]
    fn build_tree_nests_children() {
        let tree = build_tree(vec![
            proc(1, None, "init"),
            proc(10, Some(1), "sshd"),
            proc(100, Some(10), "bash"),
            proc(11, Some(1), "cron"),
        ]);
        assert_eq!(tree.len(), 4);
        assert_eq!(tree.roots.len(), 1);
        assert_eq!(tree.roots[0].children.len(), 2);
        assert_eq!(tree.roots[0].children[0].children[0].process.pid, 100);
    }

    #[test]
    fn orphans_become_roots() {
        let tree = build_tree(vec![proc(7, Some(999), "ghost")]);
        assert_eq!(tree.roots.len(), 1);
        assert_eq!(tree.roots[0].process.pid, 7);
    }

    #[test]
    fn self_parenting_does_not_loop() {
        let tree = build_tree(vec![proc(5, Some(5), "weird")]);
        assert_eq!(tree.len(), 1);
    }

    #[test]
    fn flatten_reports_depth() {
        let tree = build_tree(vec![proc(1, None, "init"), proc(2, Some(1), "child")]);
        let flat: Vec<_> = tree.iter().map(|(d, p)| (d, p.pid)).collect();
        assert_eq!(flat, vec![(0, 1), (1, 2)]);
    }

    #[test]
    fn kill_signal_parsing() {
        assert_eq!(KillSignal::parse("9"), Some(KillSignal::Kill));
        assert_eq!(KillSignal::parse("TERM"), Some(KillSignal::Terminate));
        assert_eq!(KillSignal::parse("nope"), None);
        assert!(KillSignal::Kill.is_forceful());
    }
}
