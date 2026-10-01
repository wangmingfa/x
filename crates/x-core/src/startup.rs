//! Startup items: what runs when the user logs in.
//!
//! Three OSes, three stores: Windows registry Run keys (plus the Startup
//! folder), Linux XDG autostart `.desktop` files (plus user systemd units),
//! macOS LaunchAgents. Listing is read-only; enable / disable touch the
//! store and are confirmed by the CLI.

use serde::{Deserialize, Serialize};

/// Where a startup item lives.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupSource {
    /// Windows registry `HKCU\...\Run` / `HKLM\...\Run`.
    #[default]
    RegistryRun,
    /// Windows Startup folder (`shell:startup`).
    StartupFolder,
    /// XDG `~/.config/autostart/*.desktop`.
    XdgAutostart,
    /// Linux user systemd unit (`systemctl --user`).
    SystemdUser,
    /// macOS `~/Library/LaunchAgents` (user) or `/Library/LaunchAgents`.
    LaunchAgent,
    /// macOS system daemons `/Library/LaunchDaemons`.
    LaunchDaemon,
}

impl StartupSource {
    /// Lowercase identifier.
    pub const fn name(self) -> &'static str {
        match self {
            Self::RegistryRun => "registry_run",
            Self::StartupFolder => "startup_folder",
            Self::XdgAutostart => "xdg_autostart",
            Self::SystemdUser => "systemd_user",
            Self::LaunchAgent => "launch_agent",
            Self::LaunchDaemon => "launch_daemon",
        }
    }
}

/// One program that starts at login.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StartupItem {
    /// Display / key name (`Steam`, `com.apple.Spotlight`, `myapp.desktop`).
    pub name: String,
    /// Which store it comes from.
    pub source: StartupSource,
    /// Command line as the store records it.
    pub command: Option<String>,
    /// Currently active (would run at next login).
    pub enabled: bool,
}

/// Startup item inspection and toggling.
pub trait StartupManager: Send + Sync {
    /// Everything that would start at login.
    fn list(&self) -> crate::error::Result<Vec<StartupItem>>;

    /// Enable a disabled item.
    fn enable(&self, name: &str) -> crate::error::Result<()>;

    /// Disable an item without deleting it (where the store supports that).
    fn disable(&self, name: &str) -> crate::error::Result<()>;
}
