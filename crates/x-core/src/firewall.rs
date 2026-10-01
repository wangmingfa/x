//! Firewall status and rules.
//!
//! `status` / `list` are read-only; `allow` / `deny` change the machine and
//! always need elevation, so the adapters tag the requirement and the CLI
//! confirms before running anything.

use serde::{Deserialize, Serialize};

/// Which firewall stack answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FirewallStack {
    /// Windows Firewall (netsh advfirewall).
    WindowsFirewall,
    /// nftables / iptables / ufw / firewalld on Linux.
    Netfilter,
    /// macOS pf (packet filter).
    Pf,
    /// No usable firewall found.
    Unknown,
}

impl FirewallStack {
    /// Lowercase identifier.
    pub const fn name(self) -> &'static str {
        match self {
            Self::WindowsFirewall => "windows_firewall",
            Self::Netfilter => "netfilter",
            Self::Pf => "pf",
            Self::Unknown => "unknown",
        }
    }
}

/// One firewall rule.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FirewallRule {
    /// Rule name as the stack shows it.
    pub name: String,
    /// Action (`allow` / `deny` / `reject`).
    pub action: String,
    /// Port or port range, when the rule is port scoped.
    pub port: Option<String>,
    /// Protocol (`tcp` / `udp`), when scoped.
    pub protocol: Option<String>,
    /// Enabled.
    pub enabled: bool,
}

/// Firewall inspection and rule changes.
pub trait FirewallManager: Send + Sync {
    /// Which stack answered.
    fn stack(&self) -> FirewallStack;

    /// Whether the firewall is currently enabled / active.
    fn enabled(&self) -> crate::error::Result<bool>;

    /// Visible rules (user-created first; the listing is capped by the tool).
    fn list(&self) -> crate::error::Result<Vec<FirewallRule>>;

    /// Allow a port. Needs elevation; the adapter reports the requirement.
    fn allow(
        &self,
        port: u16,
        protocol: Option<&str>,
        name: Option<&str>,
    ) -> crate::error::Result<()>;

    /// Remove / deny a previously allowed port.
    fn deny(
        &self,
        port: u16,
        protocol: Option<&str>,
        name: Option<&str>,
    ) -> crate::error::Result<()>;
}
