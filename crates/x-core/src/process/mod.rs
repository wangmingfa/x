//! Unified process capability.

pub mod manager;
pub mod model;

pub use manager::ProcessManager;
pub use model::{
    build_tree, flatten_tree, KillSignal, ProcessConnection, ProcessInfo, ProcessListOptions,
    ProcessNode, ProcessSort, ProcessState, ProcessTree,
};
