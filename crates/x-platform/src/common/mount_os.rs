//! Mount adapter: list from each OS's own view, mount/unmount via its tool.
//!
//! The unified [`MountInfo`] is derived per OS: `/proc/mounts`-style text on
//! Linux, `diskutil` / `mount` output on macOS, `Get-CimInstance
//! Win32_LogicalDisk` plus `net use` on Windows.

use std::process::Command;
use x_core::error::{Error, Result};
use x_core::mount::{MountInfo, MountManager};
use x_core::PermissionRequirement;

/// The platform mount adapter.
pub struct PlatformMount;

impl MountManager for PlatformMount {
    fn list(&self) -> Result<Vec<MountInfo>> {
        #[cfg(all(unix, not(target_os = "macos")))]
        return parse_mounts_unix(&read_file("/proc/mounts")?);
        #[cfg(target_os = "macos")]
        return macos_list();
        #[cfg(windows)]
        return windows_list();
        #[cfg(not(any(unix, windows)))]
        return Err(Error::unsupported("mount listing is not supported here"));
    }

    fn mount(&self, source: &str, target: &str) -> Result<()> {
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            run_privileged("mount", &[source, target])
        }
        #[cfg(target_os = "macos")]
        {
            // `mount -t <fs> <src> <dir>`; diskutil is the friendlier path for
            // whole disks, plain mount for arbitrary sources.
            run_privileged("mount", &[source, target])
        }
        #[cfg(windows)]
        {
            // Drive-letter mapping through `net use` (UNC shares); local
            // volumes need diskmgmt or `mountvol`, which we report honestly.
            if source.starts_with(r"\\") {
                return run_windows("net", &["use", target, source]);
            }
            Err(Error::unsupported(
                "mounting local volumes on Windows needs mountvol/admin tooling; use `x mount` for shares (\\\\server\\share) and listing",
            ))
        }
        #[cfg(not(any(unix, windows)))]
        return Err(Error::unsupported("mount is not supported here"));
    }

    fn unmount(&self, target: &str) -> Result<()> {
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            run_privileged("umount", &[target])
        }
        #[cfg(target_os = "macos")]
        {
            // diskutil unmount refuses busy volumes cleanly and names the
            // blocker, which beats umount's bare EBUSY.
            run_privileged("diskutil", &["unmount", target])
        }
        #[cfg(windows)]
        {
            if target.len() == 2 && target.ends_with(':') {
                return run_windows("net", &["use", &format!("{target} /delete")]);
            }
            run_windows("net", &["use", target, "/delete"])
        }
        #[cfg(not(any(unix, windows)))]
        return Err(Error::unsupported("unmount is not supported here"));
    }
}

#[cfg(unix)]
fn run_privileged(program: &str, args: &[&str]) -> Result<()> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| Error::system(format!("cannot run {program}: {e}")))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let mut error = Error::system(format!(
        "{program} failed: {}",
        if stderr.is_empty() {
            "non-zero exit"
        } else {
            &stderr
        }
    ));
    // mount/umount/diskutil unmount typically fail with EPERM-style messages
    // as a normal user; surface the requirement instead of the raw code.
    error = error.with_permission(PermissionRequirement::Root);
    Err(error)
}

#[cfg(windows)]
fn run_windows(program: &str, args: &[&str]) -> Result<()> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| Error::system(format!("cannot run {program}: {e}")))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let mut error = Error::system(format!(
        "{program} failed: {}",
        if stderr.is_empty() {
            "non-zero exit"
        } else {
            &stderr
        }
    ));
    error = error.with_permission(PermissionRequirement::Administrator);
    Err(error)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn read_file(path: &str) -> Result<String> {
    std::fs::read_to_string(path)
        .map_err(|e| Error::unsupported(format!("cannot read {path}: {e}")))
}

/// Parse `/proc/mounts` (or `/etc/mtab`) text.
#[cfg(all(unix, not(target_os = "macos")))]
fn parse_mounts_unix(text: &str) -> Result<Vec<MountInfo>> {
    Ok(text
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            Some(MountInfo {
                source: fields.next()?.to_string(),
                target: unescape_mount(fields.next()?),
                fs_type: fields.next()?.to_string(),
                readonly: fields
                    .next()
                    .is_some_and(|opts| opts.split(',').any(|o| o == "ro")),
                total_bytes: None,
                used_bytes: None,
            })
        })
        .collect())
}

/// macOS `mount` output: `/dev/disk3s1 on / (apfs, local, journaled)`.
#[cfg(target_os = "macos")]
fn macos_list() -> Result<Vec<MountInfo>> {
    let output = Command::new("mount")
        .output()
        .map_err(|e| Error::unsupported(format!("mount is not available: {e}")))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut rows = Vec::new();
    for line in text.lines() {
        let Some((source, rest)) = line.split_once(" on ") else {
            continue;
        };
        let Some((target, rest)) = rest.split_once(" (") else {
            continue;
        };
        let mut fields = rest.trim_end_matches(')').split(", ");
        let fs_type = fields.next().unwrap_or("unknown").to_string();
        let readonly = fields.any(|f| f == "read-only");
        rows.push(MountInfo {
            source: source.to_string(),
            target: unescape_mount(target),
            fs_type,
            readonly,
            total_bytes: None,
            used_bytes: None,
        });
    }
    Ok(rows)
}

#[cfg(windows)]
fn windows_list() -> Result<Vec<MountInfo>> {
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-CimInstance Win32_LogicalDisk | ForEach-Object { \"$($_.DeviceID)|$($_.DriveType)|$($_.FileSystem)|$($_.Size)|$($_.FreeSpace)|$($_.ProviderName)\" }",
        ])
        .output()
        .map_err(|e| Error::unsupported(format!("powershell is not available: {e}")))?;
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text
        .lines()
        .filter(|line| line.contains('|'))
        .map(|line| {
            let fields: Vec<&str> = line.trim().split('|').collect();
            let drive_type = fields.get(1).and_then(|v| v.trim().parse::<u32>().ok());
            let total = fields.get(3).and_then(|v| v.trim().parse().ok());
            let free = fields.get(4).and_then(|v| v.trim().parse().ok());
            let used = match (total, free) {
                (Some(t), Some(f)) if t >= f => Some(t - f),
                _ => None,
            };
            let network = drive_type == Some(4);
            MountInfo {
                source: fields
                    .get(5)
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| format!("{} local", fields[0].trim())),
                target: fields.first().unwrap_or(&"").trim().to_string(),
                fs_type: fields
                    .get(2)
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "unknown".to_string()),
                readonly: false,
                total_bytes: total,
                used_bytes: used,
            }
            .mark_network(network)
        })
        .collect())
}

#[cfg(windows)]
trait MarkNetwork {
    fn mark_network(self, network: bool) -> Self;
}

#[cfg(windows)]
impl MarkNetwork for MountInfo {
    fn mark_network(mut self, network: bool) -> Self {
        if network && self.fs_type == "unknown" {
            self.fs_type = "network".to_string();
        }
        self
    }
}

/// Undo the octal escapes `/proc/mounts` uses for spaces (`\040`).
#[cfg(unix)]
fn unescape_mount(target: &str) -> String {
    if target.contains("\\0") {
        target.replace("\\040", " ")
    } else {
        target.to_string()
    }
}
