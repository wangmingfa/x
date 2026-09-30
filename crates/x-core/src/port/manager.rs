//! Port capability: the single most important one.
//!
//! `x port 8080` must mean the same thing on Windows, Linux and macOS:
//! *which process is holding this port, and what would happen if I killed it.*

use crate::error::{Error, PermissionRequirement, Result};
use crate::port::model::{
    ConnectionState, KillPlan, PortInfo, PortListOptions, PortOwner, PortQuery, PortSort,
};
use std::collections::BTreeMap;

/// Platform independent port operations.
pub trait PortManager: Send + Sync {
    /// Snapshot every socket the platform can attribute to a process.
    fn list(&self, options: &PortListOptions) -> Result<Vec<PortInfo>>;

    /// Every listening socket, sorted by port.
    fn listening(&self) -> Result<Vec<PortInfo>> {
        let mut rows = self.list(&PortListOptions::listening())?;
        rows.sort_by_key(|p| (p.local_port, p.protocol));
        Ok(rows)
    }

    /// Sockets bound to `port`, any protocol, any state.
    fn find_port(&self, port: u16) -> Result<Vec<PortInfo>> {
        self.list(&PortListOptions {
            search: Some(port.to_string()),
            ..Default::default()
        })
        .map(|rows| {
            rows.into_iter()
                .filter(|p| p.local_port == port || p.remote_port == Some(port))
                .collect()
        })
    }

    /// Group sockets by owning process, sorted by port inside each owner.
    fn owners(&self, options: &PortListOptions) -> Result<Vec<PortOwner>> {
        let rows = self.list(options)?;
        Ok(group_by_owner(rows))
    }

    /// Build the exact plan required to free a port or to kill sockets by
    /// process name, without touching any process.
    ///
    /// This is what both `x port kill` and the TUI confirmation dialog run, so
    /// "confirm then kill" is implemented exactly once.
    fn plan(&self, query: &PortQuery, sort: PortSort) -> Result<KillPlan> {
        let mut plan = KillPlan::new(
            query.clone(),
            match query {
                PortQuery::Port(port) => self.find_port(*port)?,
                PortQuery::Process(name) => self.find_process(name)?,
            },
        );
        plan.sort = sort;
        Ok(plan)
    }

    /// Sockets owned by processes whose name matches `name`.
    fn find_process(&self, name: &str) -> Result<Vec<PortInfo>> {
        let rows = self.list(&PortListOptions {
            search: Some(name.to_string()),
            ..Default::default()
        })?;
        Ok(rows
            .into_iter()
            .filter(|p| {
                p.process_name
                    .as_deref()
                    .is_some_and(|n| n.to_ascii_lowercase().contains(&name.to_ascii_lowercase()))
            })
            .collect())
    }

    /// True when nothing is listening on `port`.
    fn is_free(&self, port: u16) -> Result<bool> {
        Ok(self.find_port(port)?.iter().all(|p| !p.is_listening()))
    }

    /// Privilege needed to act on a socket, e.g. killing another user's process.
    fn required_permission(&self, _rows: &[PortInfo]) -> PermissionRequirement {
        PermissionRequirement::None
    }

    /// Kill the processes owning `plan`. Returns the number of killed processes.
    ///
    /// Default implementation reports "not supported by this adapter"; adapters
    /// that can map sockets to processes override it.
    fn kill_plan(
        &self,
        _plan: &KillPlan,
        _signal: crate::process::model::KillSignal,
    ) -> Result<usize> {
        Err(Error::unsupported(
            "this platform cannot attribute sockets to processes yet",
        ))
    }
}

/// Group socket rows into per-process owners, keeping ascending port order.
pub fn group_by_owner(rows: Vec<PortInfo>) -> Vec<PortOwner> {
    let mut grouped: BTreeMap<u32, PortOwner> = BTreeMap::new();
    let mut unowned: Vec<PortInfo> = Vec::new();

    for row in rows {
        match row.pid {
            Some(pid) => {
                let owner = grouped.entry(pid).or_insert_with(|| PortOwner {
                    pid,
                    process_name: row.process_name.clone(),
                    ports: Vec::new(),
                });
                if owner.process_name.is_none() {
                    owner.process_name = row.process_name.clone();
                }
                owner.ports.push(row);
            }
            None => unowned.push(row),
        }
    }

    let mut owners: Vec<PortOwner> = grouped
        .into_values()
        .map(|mut o| {
            o.ports.sort_by_key(|a| (a.local_port, a.protocol));
            o
        })
        .collect();

    if !unowned.is_empty() {
        unowned.sort_by_key(|p| (p.local_port, p.protocol));
        owners.push(PortOwner {
            pid: 0,
            process_name: None,
            ports: unowned,
        });
    }
    owners
}

/// `true` when the row can be released by killing its owner.
pub fn is_releasable(row: &PortInfo) -> bool {
    matches!(row.state, ConnectionState::Listen | ConnectionState::Bound)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::port::model::Protocol;
    use std::net::{IpAddr, Ipv4Addr};

    fn row(pid: Option<u32>, name: Option<&str>, port: u16) -> PortInfo {
        PortInfo {
            protocol: Protocol::Tcp,
            local_address: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            local_port: port,
            remote_address: None,
            remote_port: None,
            state: ConnectionState::Listen,
            pid,
            process_name: name.map(str::to_string),
            user: None,
            path: None,
        }
    }

    #[test]
    fn owners_group_by_pid_and_sort_ports() {
        let owners = group_by_owner(vec![
            row(Some(9), Some("node"), 3000),
            row(Some(2), Some("nginx"), 80),
            row(Some(9), Some("node"), 8080),
        ]);
        assert_eq!(owners.len(), 2);
        assert_eq!(owners[0].pid, 2);
        assert_eq!(owners[1].port_numbers(), vec![3000, 8080]);
    }

    #[test]
    fn unowned_rows_are_kept_in_a_trailing_bucket() {
        let owners = group_by_owner(vec![row(Some(2), Some("nginx"), 80), row(None, None, 53)]);
        assert_eq!(owners.len(), 2);
        assert_eq!(owners[1].pid, 0);
    }

    #[test]
    fn plan_dedupes_pids() {
        let plan = KillPlan::new(
            PortQuery::Port(8080),
            vec![
                row(Some(9), Some("node"), 8080),
                row(Some(9), Some("node"), 9090),
            ],
        );
        assert_eq!(plan.target_pids(), &[9]);
        assert!(!plan.is_empty());
        assert!(plan.is_satisfiable());
    }

    #[test]
    fn plan_on_free_port_is_empty() {
        let plan = KillPlan::new(PortQuery::Port(1), vec![]);
        assert!(plan.is_empty());
        assert!(!plan.is_satisfiable());
    }

    #[test]
    fn foreign_owned_plan_requires_privileges() {
        let mut socket = row(Some(42), Some("nginx"), 80);
        socket.user = Some("root".into());
        let plan = KillPlan::new(PortQuery::Port(80), vec![socket]);
        assert_eq!(
            plan.permission(Some("alice")),
            PermissionRequirement::Elevated
        );
        assert_eq!(plan.permission(Some("root")), PermissionRequirement::None);
        assert_eq!(plan.elevated_pids(Some("root")), Vec::<u32>::new());
    }
}
