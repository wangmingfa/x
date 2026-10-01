//! Startup adapter: read the login-items stores of each OS.
//!
//! Sources per OS, in listing order:
//! - Windows: `HKCU` / `HKLM` Run keys via `reg query`, plus the Startup
//!   folder. Disable = remove the value (kept in the JSON key backup? no —
//!   disabling a registry entry deletes it; the CLI says so).
//! - Linux: `~/.config/autostart/*.desktop` (disable = `Hidden=true`),
//!   `systemctl --user` enabled units.
//! - macOS: LaunchAgents / LaunchDaemons plists (disable = rename to
//!   `.disabled`), `launchctl` state for enabled-ness.

use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
// macOS reads plists through file APIs; only the reg/systemctl paths spawn.
#[cfg(not(target_os = "macos"))]
use std::process::Command;
use x_core::error::{Error, Result};
use x_core::startup::{StartupItem, StartupManager, StartupSource};

/// The platform startup adapter.
pub struct PlatformStartup;

impl StartupManager for PlatformStartup {
    fn list(&self) -> Result<Vec<StartupItem>> {
        #[cfg(windows)]
        {
            let mut items = registry_run_items()?;
            items.extend(startup_folder_items()?);
            Ok(items)
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let mut items = xdg_autostart_items()?;
            items.extend(systemd_user_items());
            Ok(items)
        }
        #[cfg(target_os = "macos")]
        {
            let mut items = launch_plist_items(
                &home_dir().join("Library/LaunchAgents"),
                StartupSource::LaunchAgent,
            );
            items.extend(launch_plist_items(
                Path::new("/Library/LaunchAgents"),
                StartupSource::LaunchAgent,
            ));
            items.extend(launch_plist_items(
                Path::new("/Library/LaunchDaemons"),
                StartupSource::LaunchDaemon,
            ));
            Ok(items)
        }
        #[cfg(not(any(unix, windows)))]
        Err(Error::unsupported("startup listing is not supported here"))
    }

    fn enable(&self, name: &str) -> Result<()> {
        #[cfg(windows)]
        {
            // Registry Run cannot be "re-enabled" after deletion without the
            // original command; restoring is the user's job. Honest error.
            Err(Error::unsupported(format!(
                "Windows Run entry `{name}` cannot be re-enabled after deletion; \
                 re-add the program via its installer or settings"
            )))
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            // XDG autostart: flip Hidden back to false.
            let dir = autostart_dir();
            let desktop = dir.join(format!("{name}.desktop"));
            if desktop.exists() {
                let text = std::fs::read_to_string(&desktop).map_err(|e| {
                    Error::system(format!("cannot read {}: {e}", desktop.display()))
                })?;
                let cleaned: Vec<String> = text
                    .lines()
                    .map(|l| {
                        if l.trim_start().starts_with("Hidden=")
                            || l.trim_start().starts_with("X-GNOME-Autostart-enabled=")
                        {
                            "Hidden=false".to_string()
                        } else {
                            l.to_string()
                        }
                    })
                    .collect();
                std::fs::write(&desktop, cleaned.join("\n") + "\n").map_err(|e| {
                    Error::system(format!("cannot write {}: {e}", desktop.display()))
                })?;
                return Ok(());
            }
            if systemd_user_unit_exists(name) {
                return run("systemctl", &["--user", "enable", name]);
            }
            Err(Error::not_found(format!("no such startup item: {name}")))
        }
        #[cfg(target_os = "macos")]
        {
            let disabled = find_disabled_plist(name)?;
            let restored = disabled.with_extension("");
            std::fs::rename(&disabled, &restored)
                .map_err(|e| Error::system(format!("cannot rename {}: {e}", disabled.display())))?;
            Ok(())
        }
        #[cfg(not(any(unix, windows)))]
        Err(Error::unsupported("startup enable is not supported here"))
    }

    fn disable(&self, name: &str) -> Result<()> {
        #[cfg(windows)]
        {
            // HKCU first, then HKLM (HKLM needs admin; the error carries it).
            for (hive, requirement) in [
                (r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run", None),
                (
                    r"HKLM\Software\Microsoft\Windows\CurrentVersion\Run",
                    Some(x_core::PermissionRequirement::Administrator),
                ),
            ] {
                let output = Command::new("reg")
                    .args(["delete", hive, "/v", name, "/f"])
                    .output();
                if let Ok(output) = output {
                    if output.status.success() {
                        return Ok(());
                    }
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    if stderr.contains("Access is denied") {
                        let mut error = Error::system(format!(
                            "cannot delete {name} from {hive}: access denied"
                        ));
                        if let Some(requirement) = requirement {
                            error = error.with_permission(requirement);
                        }
                        return Err(error);
                    }
                }
            }
            Err(Error::not_found(format!("no Run entry named {name}")))
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            let desktop = autostart_dir().join(format!("{name}.desktop"));
            if desktop.exists() {
                let text = std::fs::read_to_string(&desktop).map_err(|e| {
                    Error::system(format!("cannot read {}: {e}", desktop.display()))
                })?;
                let mut lines: Vec<String> = text
                    .lines()
                    .filter(|l| !l.trim_start().starts_with("Hidden="))
                    .map(str::to_string)
                    .collect();
                lines.push("Hidden=true".to_string());
                std::fs::write(&desktop, lines.join("\n") + "\n").map_err(|e| {
                    Error::system(format!("cannot write {}: {e}", desktop.display()))
                })?;
                return Ok(());
            }
            if systemd_user_unit_exists(name) {
                return run("systemctl", &["--user", "disable", name]);
            }
            Err(Error::not_found(format!("no such startup item: {name}")))
        }
        #[cfg(target_os = "macos")]
        {
            for dir in [
                home_dir().join("Library/LaunchAgents"),
                PathBuf::from("/Library/LaunchAgents"),
                PathBuf::from("/Library/LaunchDaemons"),
            ] {
                let plist = dir.join(format!("{name}.plist"));
                if plist.exists() {
                    let disabled = dir.join(format!("{name}.plist.disabled"));
                    std::fs::rename(&plist, &disabled).map_err(|e| {
                        Error::system(format!("cannot rename {}: {e}", plist.display()))
                    })?;
                    return Ok(());
                }
            }
            Err(Error::not_found(format!("no launch item named {name}")))
        }
        #[cfg(not(any(unix, windows)))]
        Err(Error::unsupported("startup disable is not supported here"))
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn autostart_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    Path::new(&home).join(".config/autostart")
}

#[cfg(all(unix, not(target_os = "macos")))]
fn systemd_user_unit_exists(name: &str) -> bool {
    Command::new("systemctl")
        .args(["--user", "cat", name])
        .output()
        .is_ok_and(|o| o.status.success())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn xdg_autostart_items() -> Result<Vec<StartupItem>> {
    let dir = autostart_dir();
    let mut items = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(items);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "desktop") {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| Error::system(format!("cannot read {}: {e}", path.display())))?;
        let field = |key: &str| -> Option<String> {
            text.lines()
                .find(|l| l.trim_start().starts_with(key))
                .and_then(|l| l.split('=').nth(1))
                .map(|v| v.trim().to_string())
        };
        let hidden = field("Hidden").is_some_and(|v| v == "true");
        let disabled_flag = field("X-GNOME-Autostart-enabled").is_some_and(|v| v == "false");
        let name = field("Name").unwrap_or_else(|| {
            path.file_stem()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
        items.push(StartupItem {
            name,
            source: StartupSource::XdgAutostart,
            command: field("Exec"),
            enabled: !hidden && !disabled_flag,
        });
    }
    Ok(items)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn systemd_user_items() -> Vec<StartupItem> {
    let Ok(output) = Command::new("systemctl")
        .args(["--user", "list-unit-files", "--type=service", "--no-legend"])
        .output()
    else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let unit = fields.next()?;
            let state = fields.next()?;
            Some(StartupItem {
                name: unit.to_string(),
                source: StartupSource::SystemdUser,
                command: None,
                enabled: state.starts_with("enabled"),
            })
        })
        .collect()
}

#[cfg(windows)]
fn registry_run_items() -> Result<Vec<StartupItem>> {
    let mut items = Vec::new();
    for (hive, source) in [
        (
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run",
            StartupSource::RegistryRun,
        ),
        (
            r"HKLM\Software\Microsoft\Windows\CurrentVersion\Run",
            StartupSource::RegistryRun,
        ),
    ] {
        let Ok(output) = Command::new("reg").args(["query", hive]).output() else {
            continue;
        };
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines() {
            let trimmed = line.trim();
            if !trimmed.starts_with("REG_SZ") && !trimmed.contains("REG_SZ") {
                continue;
            }
            let mut fields = trimmed.splitn(4, ' ');
            let name = fields.next().unwrap_or_default().trim().to_string();
            let command = fields
                .last()
                .map(|c| c.trim().to_string())
                .filter(|c| !c.is_empty());
            if name.is_empty() {
                continue;
            }
            items.push(StartupItem {
                name,
                source,
                command,
                enabled: true,
            });
        }
    }
    Ok(items)
}

#[cfg(windows)]
fn startup_folder_items() -> Result<Vec<StartupItem>> {
    let mut items = Vec::new();
    let appdata = std::env::var("APPDATA").unwrap_or_default();
    if appdata.is_empty() {
        return Ok(items);
    }
    let dir = Path::new(&appdata).join(r"Microsoft\Windows\Start Menu\Programs\Startup");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(items);
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_stem().map(|n| n.to_string_lossy().into_owned());
        let Some(name) = name else { continue };
        let command = if path.extension().is_some_and(|e| e == "lnk") {
            // Shortcut targets need shell APIs; the file name is honest enough.
            Some(path.display().to_string())
        } else {
            std::fs::read_to_string(&path)
                .ok()
                .map(|t| t.lines().next().unwrap_or("").to_string())
        };
        items.push(StartupItem {
            name,
            source: StartupSource::StartupFolder,
            command,
            enabled: true,
        });
    }
    Ok(items)
}

#[cfg(target_os = "macos")]
fn launch_plist_items(dir: &Path, source: StartupSource) -> Vec<StartupItem> {
    let mut items = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return items;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_stem().map(|n| n.to_string_lossy().into_owned());
        let Some(name) = name else { continue };
        if path.extension().is_none_or(|e| e != "plist") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let program = text
            .lines()
            .skip_while(|l| {
                !l.contains("<key>Program</key>") && !l.contains("<key>ProgramArguments</key>")
            })
            .nth(1)
            .map(|l| l.trim().trim_matches(['<', '>', '/']).to_string())
            .filter(|l| !l.starts_with("key"));
        items.push(StartupItem {
            name,
            source,
            command: program,
            enabled: true,
        });
    }
    items
}

#[cfg(target_os = "macos")]
fn find_disabled_plist(name: &str) -> Result<PathBuf> {
    for dir in [
        home_dir().join("Library/LaunchAgents"),
        PathBuf::from("/Library/LaunchAgents"),
        PathBuf::from("/Library/LaunchDaemons"),
    ] {
        let candidate = dir.join(format!("{name}.plist.disabled"));
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(Error::not_found(format!(
        "no disabled launch item named {name}"
    )))
}

#[cfg(target_os = "macos")]
fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/Users/unknown"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn run(program: &str, args: &[&str]) -> Result<()> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| Error::system(format!("cannot run {program}: {e}")))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(Error::system(format!(
        "{program} failed: {}",
        if stderr.is_empty() {
            "non-zero exit"
        } else {
            &stderr
        }
    )))
}
