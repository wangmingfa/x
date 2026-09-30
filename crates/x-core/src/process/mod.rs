//! Unified process capability.

pub mod manager;
pub mod model;

pub use manager::ProcessManager;
pub use model::{
    build_tree, flatten_tree, KillSignal, ProcessInfo, ProcessListOptions, ProcessNode,
    ProcessSort, ProcessState, ProcessTree,
};
