//! macOS service adapter backed by launchd.
//!
//! `launchctl` is the documented interface to launchd and is treated as the
//! Level 3 fallback of the native-first policy: there is no stable C API for
//! listing user agents, only the private `launchd` XPC protocol.

use crate::sys;
use x_core::error::{Error, PermissionRequirement, Result};
use x_core::service::{
    NativeOutput, ServiceAction, ServiceInfo, ServiceListOptions, ServiceLogEntry, ServiceLogPage,
    ServiceManager, ServiceManagerType, ServiceState, DEFAULT_LOG_LINES,
};

/// Reads and controls launchd jobs.
#[derive(Debug, Default)]
pub struct MacosService {
    domain: String,
}

impl MacosService {
    /// Create the adapter.
    pub fn new() -> Self {
        Self {
            domain: "system".to_string(),
        }
    }

    /// Create the adapter for the per-user launchd domain.
    pub fn user_domain() -> Self {
        Self {
            domain: format!(
                "gui/{}",
                crate::sys::current_user_name().unwrap_or_else(|| "0".into())
            ),
        }
    }

    /// Create the adapter for a specific launchd domain.
    pub fn with_domain(domain: impl Into<String>) -> Self {
        Self {
            domain: domain.into(),
        }
    }

    /// Domain this adapter talks to.
    pub fn domain(&self) -> &str {
        &self.domain
    }
}

impl ServiceManager for MacosService {
    fn manager_type(&self) -> ServiceManagerType {
        if sys::command_exists("launchctl") {
            ServiceManagerType::Launchd
        } else {
            ServiceManagerType::Unknown
        }
    }

    fn list(&self, options: &ServiceListOptions) -> Result<Vec<ServiceInfo>> {
        let output = sys::run_command("launchctl", &["list"])
            .map_err(|e| Error::system(format!("cannot list launchd jobs: {e}")))?;

        let mut rows = Vec::new();
        for line in output.lines() {
            let cols: Vec<&str> = line.split_whitespace().collect();
            if cols.len() < 3 {
                continue;
            }
            let pid = cols[0].parse::<u32>().ok().filter(|p| *p > 0);
            let name = cols[cols.len() - 1].to_string();

            let row = ServiceInfo {
                display_name: None,
                description: None,
                name,
                state: if pid.is_some() {
                    ServiceState::Running
                } else {
                    // `launchctl list` prints "-" in the PID column when the job
                    // is loaded but not running.
                    ServiceState::Stopped
                },
                pid,
                enabled: None,
                manager: ServiceManagerType::Launchd,
            };

            if let Some(state) = options.state {
                if row.state != state {
                    continue;
                }
            }
            if options.running_only && !row.state.is_running() {
                continue;
            }
            if let Some(term) = options.search.as_deref() {
                let term = term.to_ascii_lowercase();
                if !row.name.to_ascii_lowercase().contains(&term) {
                    continue;
                }
            }
            rows.push(row);
        }

        rows.sort_by(|a, b| a.name.cmp(&b.name));
        if let Some(limit) = options.limit {
            rows.truncate(limit);
        }
        Ok(rows)
    }

    fn status(&self, name: &str) -> Result<ServiceInfo> {
        let label = normalize_label(name);
        let output = sys::run_command("launchctl", &["print", &format!("{}/{label}", self.domain)])
            .map_err(|_| Error::not_found(format!("launchd job `{name}` not found")))?;

        let running = output.contains("state = running") || output.contains("pid = ");
        Ok(ServiceInfo {
            name: label,
            display_name: None,
            description: None,
            state: if running {
                ServiceState::Running
            } else {
                ServiceState::Stopped
            },
            pid: output
                .lines()
                .find_map(|l| l.trim().strip_prefix("pid = "))
                .and_then(|v| v.trim().parse().ok()),
            enabled: None,
            manager: ServiceManagerType::Launchd,
        })
    }

    fn action(&self, name: &str, action: ServiceAction) -> Result<()> {
        let label = normalize_label(name);
        let target = format!("{}/{label}", self.domain);
        let verb = match action {
            ServiceAction::Start => "bootstrap",
            ServiceAction::Stop => "bootout",
            ServiceAction::Restart => "kickstart",
            ServiceAction::Reload => "kickstart",
            ServiceAction::Enable => "enable",
            ServiceAction::Disable => "disable",
        };

        let args: Vec<String> = match action {
            ServiceAction::Start | ServiceAction::Stop => vec![verb.into(), target.clone()],
            ServiceAction::Restart | ServiceAction::Reload => {
                vec![verb.into(), "-k".into(), target.clone()]
            }
            ServiceAction::Enable | ServiceAction::Disable => {
                vec![verb.into(), target.clone()]
            }
        };
        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();

        match sys::run_command("launchctl", &borrowed) {
            Ok(_) => Ok(()),
            Err(e) => {
                let text = e.to_string();
                if text.contains("Operation not permitted")
                    || text.contains("Could not service target")
                    || text.contains("not authorized")
                {
                    Err(Error::permission_denied(
                        PermissionRequirement::Administrator,
                        format!("launchd refused `{verb}` on `{label}`"),
                    ))
                } else if text.contains("Could not find service") || text.contains("not found") {
                    Err(Error::not_found(format!("launchd job `{label}` not found")))
                } else {
                    Err(Error::system(format!("launchctl {verb} failed: {e}")))
                }
            }
        }
    }

    fn logs(&self, name: &str, limit: Option<usize>) -> Result<ServiceLogPage> {
        let process = name.trim();
        if process.is_empty() {
            return Err(Error::invalid_input("service name must not be empty"));
        }
        // The name goes inside a quoted predicate string; a quote or backslash
        // there would change the query itself.
        if process.contains(['"', '\'', '\\']) {
            return Err(Error::invalid_input(format!(
                "`{process}` cannot be used as a log predicate value"
            )));
        }
        if !sys::command_exists("log") {
            return Err(Error::unsupported("`log` is not available on this host"));
        }
        // The unified log is keyed by executable name, not launchd label:
        // `sshd`, not `com.openssh.sshd`. There is no per-job window, so the
        // last hour is the documented bound.
        let predicate = format!("process == \"{process}\"");
        let output = sys::run_command(
            "log",
            &[
                "show",
                "--predicate",
                &predicate,
                "--last",
                "1h",
                "--style",
                "compact",
                "--info",
            ],
        )
        .map_err(|err| Error::system(format!("log show: {err}")))?;

        let lines = limit.unwrap_or(DEFAULT_LOG_LINES);
        let mut entries: Vec<ServiceLogEntry> = output
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(compact_log_line)
            .collect();
        // `log show` prints oldest first; a page is newest first.
        entries.reverse();
        entries.truncate(lines);
        Ok(ServiceLogPage {
            service: process.to_string(),
            source: "log show".to_string(),
            entries,
        })
    }

    fn native(&self, args: &[String]) -> Result<NativeOutput> {
        if args.is_empty() {
            return Err(Error::invalid_input(
                "x service native needs at least one manager argument",
            ));
        }
        sys::run_native_lossy("launchctl", args)
            .map_err(|err| Error::system(format!("cannot run `launchctl`: {err}")))
    }
}

/// One `log show --style compact` line: date, time, then the record body.
fn compact_log_line(line: &str) -> ServiceLogEntry {
    let mut rest = line;
    let mut stamp = String::new();
    for _ in 0..2 {
        let trimmed = rest.trim_start();
        let end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
        if end == 0 {
            break;
        }
        if !stamp.is_empty() {
            stamp.push(' ');
        }
        stamp.push_str(&trimmed[..end]);
        rest = &trimmed[end..];
    }
    // The date token is a guaranteed shape; anything else (a header or
    // wrapped line) keeps its full text as the message.
    let looks_like_timestamp = stamp.len() >= 20 && stamp.as_bytes()[0].is_ascii_digit();
    if !looks_like_timestamp {
        return ServiceLogEntry {
            timestamp: None,
            level: None,
            message: line.to_string(),
        };
    }
    ServiceLogEntry {
        timestamp: Some(stamp),
        level: None,
        message: rest.trim_start().to_string(),
    }
}

/// `com.apple.something` style labels, with a friendly alias for common jobs.
fn normalize_label(name: &str) -> String {
    match name {
        "sshd" | "ssh" => "com.openssh.sshd".to_string(),
        "nginx" => "homebrew.mxcl.nginx".to_string(),
        "postgres" | "postgresql" => "homebrew.mxcl.postgresql".to_string(),
        "redis" => "homebrew.mxcl.redis".to_string(),
        other => other.to_string(),
    }
}

/// Trait object helper.
pub fn manager() -> std::sync::Arc<dyn ServiceManager> {
    std::sync::Arc::new(MacosService::new())
}

/// Default adapter.
pub fn as_manager() -> std::sync::Arc<dyn ServiceManager> {
    manager()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_launchd() {
        assert_eq!(
            MacosService::new().manager_type(),
            ServiceManagerType::Launchd
        );
    }

    #[test]
    fn lists_loaded_jobs() {
        let rows = MacosService::new()
            .list(&ServiceListOptions::default())
            .expect("list");
        assert!(!rows.is_empty(), "launchctl list returned nothing");
        assert!(rows
            .iter()
            .all(|r| r.manager == ServiceManagerType::Launchd));
        assert!(rows.iter().all(|r| !r.name.is_empty()));
    }

    #[test]
    fn running_filter_matches_state() {
        let rows = MacosService::new()
            .list(&ServiceListOptions {
                running_only: true,
                ..Default::default()
            })
            .expect("list");
        assert!(rows.iter().all(|r| r.state.is_running()));
        assert!(rows.iter().all(|r| r.pid.is_some()));
    }

    #[test]
    fn search_filter_narrows_the_list() {
        let all = MacosService::new()
            .list(&ServiceListOptions::default())
            .expect("list");
        let Some(first) = all.first() else {
            return;
        };
        let needle = first.name.chars().take(4).collect::<String>();
        let filtered = MacosService::new()
            .list(&ServiceListOptions {
                search: Some(needle.clone()),
                ..Default::default()
            })
            .expect("list");
        assert!(filtered.iter().all(|r| r.name.contains(&needle)));
        assert!(filtered.len() <= all.len());
    }

    #[test]
    fn unknown_job_is_not_found() {
        let err = MacosService::new()
            .status("com.example.definitely.missing")
            .expect_err("must fail");
        assert_eq!(err.kind(), x_core::ErrorKind::NotFound);
    }

    #[test]
    fn labels_are_normalized() {
        assert_eq!(normalize_label("sshd"), "com.openssh.sshd");
        assert_eq!(normalize_label("nginx"), "homebrew.mxcl.nginx");
        assert_eq!(normalize_label("custom.daemon"), "custom.daemon");
    }

    #[test]
    fn compact_log_lines_split_the_timestamp() {
        let entry = compact_log_line(
            "2026-10-01 12:00:00.123456+0800 0x5f3a  Default  0x0  123  0  launchd: message",
        );
        assert_eq!(
            entry.timestamp.as_deref(),
            Some("2026-10-01 12:00:00.123456+0800")
        );
        assert!(entry.message.starts_with("0x5f3a"));
        // Header and wrapped lines keep their full text.
        let header = compact_log_line("Filtering the log data that is being inferred.");
        assert_eq!(header.timestamp, None);
        assert_eq!(
            header.message,
            "Filtering the log data that is being inferred."
        );
    }

    #[test]
    fn log_predicate_values_are_injected_safely() {
        let service = MacosService::new();
        let err = service
            .logs("evil\" or 1=1", None)
            .expect_err("must be rejected");
        assert_eq!(err.kind(), x_core::error::ErrorKind::InvalidInput);
        assert!(service.logs("", None).is_err());
    }

    #[test]
    fn native_refuses_an_empty_argument_list() {
        let err = MacosService::new().native(&[]).expect_err("must reject");
        assert_eq!(err.kind(), x_core::error::ErrorKind::InvalidInput);
    }
}
