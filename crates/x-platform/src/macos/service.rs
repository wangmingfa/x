//! macOS service adapter backed by launchd.
//!
//! `launchctl` is the documented interface to launchd and is treated as the
//! Level 3 fallback of the native-first policy: there is no stable C API for
//! listing user agents, only the private `launchd` XPC protocol.

use crate::sys;
use x_core::error::{Error, PermissionRequirement, Result};
use x_core::service::{
    ServiceAction, ServiceInfo, ServiceListOptions, ServiceManager, ServiceManagerType,
    ServiceState,
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
}
