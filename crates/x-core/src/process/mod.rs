//! Unified process capability.

pub mod manager;
pub mod model;

pub use manager::ProcessManager;
pub use model::{
    build_tree, diff_processes, flatten_tree, KillSignal, ProcessConnection, ProcessDiff,
    ProcessInfo, ProcessListOptions, ProcessNode, ProcessSort, ProcessState, ProcessTree,
};
