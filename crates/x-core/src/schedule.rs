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

    /// Remove by name after re-reading the store and checking the entry
    /// still matches `expect` (the command column `list` showed).
    ///
    /// This closes the list→remove race: crontab lines are addressed by
    /// number, and whatever moved onto that line between `list` and
    /// `remove` is refused instead of silently deleted.
    fn remove_checked(&self, name: &str, expect: &str) -> crate::error::Result<()> {
        let entry = self
            .list()?
            .into_iter()
            .find(|entry| entry.name == name)
            .ok_or_else(|| {
                crate::error::Error::not_found(format!("no scheduled task named {name}"))
            })?;
        if entry.command != expect {
            return Err(crate::error::Error::new(
                crate::error::ErrorKind::InvalidState,
                format!(
                    "{name} changed since it was listed (now: {}); re-run list and retry",
                    entry.command
                ),
            ));
        }
        self.remove(name)
    }
}

#[cfg(test)]
mod checked_tests {
    use super::*;
    use crate::error::{Error, ErrorKind, Result};
    use std::sync::Mutex;

    /// A store whose current contents can change between `list` and
    /// `remove`, exactly like a crontab can.
    struct VolatileStore {
        rows: Mutex<Vec<ScheduleEntry>>,
        removed: Mutex<Vec<String>>,
    }

    impl ScheduleManager for VolatileStore {
        fn list(&self) -> Result<Vec<ScheduleEntry>> {
            Ok(self.rows.lock().expect("mutex").clone())
        }
        fn add(&self, _name: &str, _schedule: &str, _command: &str) -> Result<()> {
            unreachable!()
        }
        fn remove(&self, name: &str) -> Result<()> {
            let mut rows = self.rows.lock().expect("mutex");
            let index = rows
                .iter()
                .position(|entry| entry.name == name)
                .ok_or_else(|| Error::not_found(format!("no task {name}")))?;
            rows.remove(index);
            self.removed.lock().expect("mutex").push(name.to_string());
            Ok(())
        }
    }

    fn entry(name: &str, command: &str) -> ScheduleEntry {
        ScheduleEntry {
            name: name.to_string(),
            command: command.to_string(),
            trigger: None,
            enabled: true,
        }
    }

    #[test]
    fn a_matching_entry_is_removed() {
        let store = VolatileStore {
            rows: Mutex::new(vec![entry("crontab:3", "daily-backup")]),
            removed: Mutex::new(Vec::new()),
        };
        store.remove_checked("crontab:3", "daily-backup").unwrap();
        assert_eq!(store.removed.lock().expect("mutex").len(), 1);
    }

    #[test]
    fn a_changed_line_is_refused_not_deleted() {
        // Between `list` and `remove` another line landed on the address:
        // the store now shows different content under the same name.
        let store = VolatileStore {
            rows: Mutex::new(vec![entry("crontab:3", "rm -rf /important")]),
            removed: Mutex::new(Vec::new()),
        };
        let err = store
            .remove_checked("crontab:3", "daily-backup")
            .expect_err("the changed line must be refused");
        assert_eq!(err.kind(), ErrorKind::InvalidState, "{err}");
        assert!(
            store.removed.lock().expect("mutex").is_empty(),
            "nothing was removed"
        );
    }

    #[test]
    fn a_disappeared_entry_is_not_found() {
        let store = VolatileStore {
            rows: Mutex::new(Vec::new()),
            removed: Mutex::new(Vec::new()),
        };
        let err = store
            .remove_checked("crontab:3", "daily-backup")
            .expect_err("an absent entry cannot be removed");
        assert_eq!(err.kind(), ErrorKind::NotFound, "{err}");
    }
}
