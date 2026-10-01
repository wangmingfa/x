//! Scheduled tasks / jobs.
//!
//! Windows Task Scheduler, Linux cron (plus systemd timers as read-only
//! visibility) and macOS launchd user jobs share one small model. `add` /
//! `remove` write to the platform's own store and are confirmed by the CLI;
//! the platform tools used are the ones a sysadmin would run by hand.

use serde::{Deserialize, Serialize};

/// When a scheduled task runs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScheduleTrigger {
    /// Free-form schedule as the platform spells it (`*/5 * * * *`,
    /// `Every 5 minutes`, `Daily at 09:00`).
    pub spec: String,
}

/// One scheduled task.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ScheduleEntry {
    /// Task name (unique within its store).
    pub name: String,
    /// Command the task runs.
    pub command: String,
    /// When it runs.
    pub trigger: Option<ScheduleTrigger>,
    /// Currently scheduled / enabled.
    pub enabled: bool,
}

/// Scheduled task management.
pub trait ScheduleManager: Send + Sync {
    /// Everything scheduled for this user (or the machine, where the tool
    /// mixes both — the entry says which it saw).
    fn list(&self) -> crate::error::Result<Vec<ScheduleEntry>>;

    /// Create (or overwrite) a task.
    ///
    /// `schedule` is cron syntax on Linux, a Task Scheduler `/sc` style
    /// description on Windows, and an `EveryNMinutes` style interval on macOS.
    /// Each adapter documents the exact grammar it accepts.
    fn add(&self, name: &str, schedule: &str, command: &str) -> crate::error::Result<()>;

    /// Remove a task by name.
    fn remove(&self, name: &str) -> crate::error::Result<()>;
}
