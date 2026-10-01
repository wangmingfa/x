//! Power and clock.
//!
//! Queries (battery, current time, time zone) are read-only facts. The verbs
//! (`sleep`, `shutdown`, `reboot`, `sync`) change the machine or need
//! privileges, so they go through a confirmation in the CLI and audited
//! platform commands.

use serde::{Deserialize, Serialize};

/// Battery status, when the machine has one.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BatteryInfo {
    /// A battery is present.
    pub present: bool,
    /// 0–100, when the OS reports it.
    pub percent: Option<f32>,
    /// Charging / discharging / full / unknown.
    pub state: Option<String>,
    /// Estimated seconds to empty (discharging) or full (charging).
    pub seconds_remaining: Option<u64>,
    /// AC power connected, when detectable.
    pub plugged: Option<bool>,
}

/// Power management verbs plus battery facts.
pub trait PowerManager: Send + Sync {
    /// Battery status; `present: false` on desktops and VMs.
    fn battery(&self) -> crate::error::Result<BatteryInfo> {
        Ok(BatteryInfo::default())
    }

    /// Put the machine to sleep. Blocking until the OS accepts the request.
    fn sleep(&self) -> crate::error::Result<()>;

    /// Shut the machine down after `delay_seconds`.
    fn shutdown(&self, delay_seconds: u32) -> crate::error::Result<()>;

    /// Restart the machine after `delay_seconds`.
    fn reboot(&self, delay_seconds: u32) -> crate::error::Result<()>;
}

/// Current clock facts, assembled without platform help.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimeInfo {
    /// Local wall clock, RFC 3339 with offset if available.
    pub local: String,
    /// UTC wall clock, RFC 3339.
    pub utc: String,
    /// Unix seconds.
    pub unix_seconds: i64,
}

/// Collect current clock facts.
pub fn time_info() -> TimeInfo {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    TimeInfo {
        local: crate::audit::now_utc_rfc3339(),
        utc: crate::audit::rfc3339_utc(now),
        unix_seconds: now,
    }
}
