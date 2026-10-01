//! Power adapter: battery facts and the sleep / shutdown / reboot verbs.
//!
//! Every verb goes through the OS's own command; each arm documents what runs.
//! The CLI adds confirmation on top — these are the most destructive commands
//! in the whole tool.

use x_core::error::{Error, Result};
use x_core::power::{BatteryInfo, PowerManager};
use x_core::PermissionRequirement;

/// The platform power adapter.
pub struct PlatformPower;

impl PowerManager for PlatformPower {
    fn battery(&self) -> Result<BatteryInfo> {
        #[cfg(target_os = "macos")]
        {
            macos_battery()
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            linux_battery()
        }
        #[cfg(windows)]
        {
            windows_battery()
        }
        #[cfg(not(any(unix, windows)))]
        Ok(BatteryInfo::default())
    }

    fn sleep(&self) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            run("pmset", &["sleepnow"], PermissionRequirement::None)
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            run("systemctl", &["suspend"], PermissionRequirement::None)
        }
        #[cfg(windows)]
        {
            // 0,1,0 = suspend, not hibernate; second flag forces it even when
            // a program objects, which is what "sleep now" means here.
            let output = Command::new("rundll32.exe")
                .args(["powrprof.dll,SetSuspendState", "0,1,0"])
                .output()
                .map_err(|e| Error::system(format!("cannot run rundll32: {e}")))?;
            if output.status.success() {
                Ok(())
            } else {
                Err(Error::system("sleep request was refused"))
            }
        }
        #[cfg(not(any(unix, windows)))]
        Err(Error::unsupported("sleep is not supported here"))
    }

    fn shutdown(&self, delay_seconds: u32) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            let minutes = (delay_seconds / 60).max(1);
            run(
                "shutdown",
                &["-h", &format!("+{minutes}")],
                PermissionRequirement::Root,
            )
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let minutes = (delay_seconds / 60).max(1);
            run(
                "shutdown",
                &["-h", &format!("+{minutes}")],
                PermissionRequirement::Root,
            )
        }
        #[cfg(windows)]
        {
            run(
                "shutdown",
                &["/s", "/t", &delay_seconds.to_string()],
                PermissionRequirement::Administrator,
            )
        }
        #[cfg(not(any(unix, windows)))]
        Err(Error::unsupported("shutdown is not supported here"))
    }

    fn reboot(&self, delay_seconds: u32) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            let minutes = (delay_seconds / 60).max(1);
            run(
                "shutdown",
                &["-r", &format!("+{minutes}")],
                PermissionRequirement::Root,
            )
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let minutes = (delay_seconds / 60).max(1);
            run(
                "shutdown",
                &["-r", &format!("+{minutes}")],
                PermissionRequirement::Root,
            )
        }
        #[cfg(windows)]
        {
            run(
                "shutdown",
                &["/r", "/t", &delay_seconds.to_string()],
                PermissionRequirement::Administrator,
            )
        }
        #[cfg(not(any(unix, windows)))]
        Err(Error::unsupported("reboot is not supported here"))
    }
}

fn run(program: &str, args: &[&str], requirement: x_core::PermissionRequirement) -> Result<()> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| Error::system(format!("cannot run {program}: {e}")))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    // Permission failures get the structured requirement so the CLI can show
    // actionable guidance (sudo / run as Administrator).
    let denied = output.status.code() == Some(1)
        && (stderr.contains("denied") || stderr.contains("refused") || stderr.is_empty());
    let mut error = Error::system(format!(
        "{program} failed: {}",
        if stderr.is_empty() {
            "exit code non-zero"
        } else {
            &stderr
        }
    ));
    if denied {
        error = error.with_permission(requirement);
    }
    Err(error)
}

#[cfg(target_os = "macos")]
fn macos_battery() -> Result<BatteryInfo> {
    let output = Command::new("pmset")
        .arg("-g")
        .arg("batt")
        .output()
        .map_err(|e| Error::system(format!("cannot run pmset: {e}")))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut info = BatteryInfo {
        present: text.contains("Battery") || text.contains("InternalBattery"),
        ..BatteryInfo::default()
    };
    if let Some(line) = text.lines().find(|l| l.contains('%')) {
        if let Some(percent) = line.split_whitespace().find(|w| w.ends_with('%')) {
            info.percent = percent.trim_end_matches('%').parse().ok();
        }
        info.state = Some(
            if line.contains("discharging") {
                "discharging"
            } else if line.contains("charging") {
                "charging"
            } else if line.contains("AC Power") {
                "full"
            } else {
                "unknown"
            }
            .to_string(),
        );
        info.plugged = Some(line.contains("AC Power"));
    }
    Ok(info)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn linux_battery() -> Result<BatteryInfo> {
    // First battery the kernel exposes; present: false when none exists.
    let base = std::path::Path::new("/sys/class/power_supply");
    let Some(bat) = std::fs::read_dir(base)
        .map(|entries| {
            entries
                .flatten()
                .find(|e| e.file_name().to_string_lossy().starts_with("BAT"))
        })
        .unwrap_or(None)
    else {
        return Ok(BatteryInfo::default());
    };
    let read = |name: &str| -> Option<String> {
        std::fs::read_to_string(bat.path().join(name))
            .ok()
            .map(|s| s.trim().to_string())
    };
    let percent = read("capacity").and_then(|s| s.parse().ok());
    let status = read("status");
    let plugged = read("online").map(|s| s == "1");
    Ok(BatteryInfo {
        present: true,
        percent,
        state: status,
        seconds_remaining: None,
        plugged,
    })
}

#[cfg(windows)]
fn windows_battery() -> Result<BatteryInfo> {
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-CimInstance Win32_Battery | ConvertTo-Json -Compress",
        ])
        .output()
        .map_err(|e| Error::system(format!("cannot run powershell: {e}")))?;
    let text = String::from_utf8_lossy(&output.stdout);
    if text.trim().is_empty() {
        return Ok(BatteryInfo::default());
    }
    // Minimal field scrape, no serde-json dependency here.
    let field = |name: &str| -> Option<String> {
        text.split(',')
            .find(|pair| pair.contains(name))
            .and_then(|pair| pair.split(':').nth(1))
            .map(|v| v.trim().trim_matches(['"', '}']).to_string())
            .filter(|v| !v.is_empty())
    };
    let percent = field("EstimatedChargeRemaining").and_then(|v| v.parse().ok());
    let state_code = field("BatteryStatus").and_then(|v| v.parse::<u32>().ok());
    let state = state_code.map(|code| match code {
        2 => "charging".to_string(),
        1 => "discharging".to_string(),
        _ => "unknown".to_string(),
    });
    Ok(BatteryInfo {
        present: true,
        percent,
        state,
        seconds_remaining: None,
        plugged: state_code.map(|code| code == 2),
    })
}

use std::process::Command;
