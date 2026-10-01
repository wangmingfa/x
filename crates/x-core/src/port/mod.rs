//! Unified port capability.

pub mod manager;
pub mod model;

pub use manager::{group_by_owner, is_releasable, PortManager};
pub use model::{
    diff_sockets, summarize, ConnectionState, KillPlan, PortInfo, PortListOptions, PortOwner,
    PortQuery, PortSort, PortStats, Protocol, QueueStats, SocketDiff,
};
