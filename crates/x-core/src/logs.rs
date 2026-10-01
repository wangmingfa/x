//! System log queries: one read-only surface over three very different stores.
//!
//! journald, the Windows Event Log and the macOS unified log share no level
//! vocabulary, no timestamp format and no scope model. Like
//! [`crate::service::ServiceLogEntry`], the records below pass the platform's
//! own text through instead of inventing a lossy normalisation.
//!
//! Everything here is a read: no audit entry, no confirmation, no elevation —
//! except where the platform's own store refuses an unprivileged reader
//! (journald access groups), which the adapter reports as a plain error.

use serde::{Deserialize, Serialize};

/// Which slice of the system log to read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogScope {
    /// The machine-wide log (journal, System event channel, unified log).
    System,
    /// Records attributed to one service (unit name, event provider, process).
    Service(String),
    /// Records attributed to one process: pid or program name.
    Process(String),
}

impl LogScope {
    /// Lowercase identifier with the selector, e.g. `service:nginx`.
    pub fn label(&self) -> String {
        match self {
            Self::System => "system".to_string(),
            Self::Service(name) => format!("service:{name}"),
            Self::Process(target) => format!("process:{target}"),
        }
    }
}

/// One log record, as the platform printed it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LogEntry {
    /// Timestamp text, when the record carried one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    /// Level text ("err", "错误", "Default"), when the record carried one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    /// Originating unit / provider / process, when the record carried one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// The message text.
    pub message: String,
}

/// One page of log records, newest first.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LogPage {
    /// Which scope was asked for.
    pub scope: String,
    /// Where the entries came from, e.g. `journalctl` or `Get-WinEvent`.
    pub source: String,
    /// Records, newest first.
    pub entries: Vec<LogEntry>,
}

/// Read-only access to the platform's system log store.
pub trait LogReader: Send + Sync {
    /// Fetch up to `limit` newest records for `scope`.
    fn read(&self, scope: &LogScope, limit: usize) -> crate::error::Result<LogPage>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_labels_carry_the_selector() {
        assert_eq!(LogScope::System.label(), "system");
        assert_eq!(LogScope::Service("nginx".into()).label(), "service:nginx");
        assert_eq!(LogScope::Process("4242".into()).label(), "process:4242");
    }

    #[test]
    fn entries_serialize_minimally() {
        let entry = LogEntry {
            message: "boom".to_string(),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_string(&entry).expect("json"),
            "{\"message\":\"boom\"}"
        );
    }
}
