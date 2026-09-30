//! Unified service model.
//!
//! The interesting part is that a service is *not* the same concept on every
//! platform: Windows has the SCM, Linux has systemd, OpenRC, runit or nothing
//! at all, macOS has launchd. x exposes one vocabulary and reports which
//! manager backs it.

use serde::{Deserialize, Serialize};

/// Which service manager the adapter actually found at runtime.
///
/// Adapters must detect this instead of assuming: plenty of Linux containers
/// run without systemd.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceManagerType {
    /// Windows Service Control Manager.
    #[cfg_attr(not(windows), allow(dead_code))]
    #[serde(rename = "windows_scm")]
    WindowsScm,
    /// systemd (`systemctl` / D-Bus).
    #[serde(rename = "systemd")]
    Systemd,
    /// OpenRC (`rc-service`).
    #[serde(rename = "openrc")]
    OpenRc,
    /// SysV init scripts.
    #[serde(rename = "sysv")]
    SysV,
    /// Upstart.
    #[serde(rename = "upstart")]
    Upstart,
    /// macOS launchd (`launchctl`).
    #[serde(rename = "launchd")]
    Launchd,
    /// Nothing was detected.
    Unknown,
}

impl ServiceManagerType {
    /// Lowercase identifier.
    pub const fn name(self) -> &'static str {
        match self {
            Self::WindowsScm => "windows_scm",
            Self::Systemd => "systemd",
            Self::OpenRc => "openrc",
            Self::SysV => "sysv",
            Self::Upstart => "upstart",
            Self::Launchd => "launchd",
            Self::Unknown => "unknown",
        }
    }

    /// Whether lifecycle operations are supported by this manager.
    pub const fn supports_control(self) -> bool {
        !matches!(self, Self::Unknown)
    }
}

/// Desired or effective state of a service.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceState {
    /// Running now.
    Running,
    /// Installed but not running.
    Stopped,
    /// Starting.
    Starting,
    /// Stopping.
    Stopping,
    /// Failed to start or crashed.
    Failed,
    /// Installed but disabled at boot.
    Disabled,
    /// Installed and enabled at boot, not running now.
    Enabled,
    /// Enabled at boot and running.
    ActiveEnabled,
    /// Not installed.
    NotFound,
    /// The manager does not report a state.
    Unknown,
}

impl ServiceState {
    /// `true` when the service is currently running.
    pub const fn is_running(self) -> bool {
        matches!(self, Self::Running | Self::ActiveEnabled)
    }
}

/// Lifecycle verbs accepted by every service manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServiceAction {
    /// Start a stopped service.
    Start,
    /// Stop a running service.
    Stop,
    /// Stop then start.
    Restart,
    /// Reload configuration without restarting.
    Reload,
    /// Enable at boot.
    Enable,
    /// Disable at boot.
    Disable,
}

/// A service as exposed by the platform manager.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceInfo {
    /// Identifier accepted by `status`, `start`, ... on this platform.
    pub name: String,
    /// Display name when the platform has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// Description when the platform has one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Current state.
    pub state: ServiceState,
    /// Main process id when running.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Whether the service starts at boot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Manager backing this entry.
    pub manager: ServiceManagerType,
}

/// Filter for [`crate::service::ServiceManager::list`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServiceListOptions {
    /// Only services whose name contains this substring.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search: Option<String>,
    /// Only services in this state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<ServiceState>,
    /// Only running services.
    pub running_only: bool,
    /// Maximum number of rows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

/// Service capability, normalized across platforms.
pub trait ServiceManager: Send + Sync {
    /// Which manager backs this adapter.
    fn manager_type(&self) -> ServiceManagerType;

    /// List services.
    fn list(&self, options: &ServiceListOptions) -> crate::error::Result<Vec<ServiceInfo>>;

    /// Look up one service.
    fn status(&self, name: &str) -> crate::error::Result<ServiceInfo> {
        let options = ServiceListOptions {
            search: Some(name.to_string()),
            ..Default::default()
        };
        self.list(&options)?
            .into_iter()
            .find(|s| s.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| crate::error::Error::not_found(format!("service `{name}` not found")))
    }

    /// Apply a lifecycle action.
    fn action(&self, name: &str, action: ServiceAction) -> crate::error::Result<()>;

    /// Convenience wrappers so adapters only implement `action`.
    fn start(&self, name: &str) -> crate::error::Result<()> {
        self.action(name, ServiceAction::Start)
    }

    /// See [`ServiceManager::start`].
    fn stop(&self, name: &str) -> crate::error::Result<()> {
        self.action(name, ServiceAction::Stop)
    }

    /// See [`ServiceManager::start`].
    fn restart(&self, name: &str) -> crate::error::Result<()> {
        self.action(name, ServiceAction::Restart)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manager_names_are_stable() {
        assert_eq!(ServiceManagerType::Systemd.name(), "systemd");
        assert_eq!(ServiceManagerType::Launchd.name(), "launchd");
        assert!(ServiceManagerType::Systemd.supports_control());
        assert!(!ServiceManagerType::Unknown.supports_control());
    }

    #[test]
    fn state_predicates() {
        assert!(ServiceState::Running.is_running());
        assert!(ServiceState::ActiveEnabled.is_running());
        assert!(!ServiceState::Stopped.is_running());
    }

    #[test]
    fn manager_type_serde_is_snake_case() {
        let json = serde_json::to_string(&ServiceManagerType::OpenRc).unwrap();
        assert_eq!(json, "\"openrc\"");
    }
}
