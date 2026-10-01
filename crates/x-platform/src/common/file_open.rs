//! File open / reveal / trash, one adapter selected per target OS.
//!
//! `x-core` owns the inspection verbs (they are plain `std::fs`); what lives
//! here is the part where the three OSes genuinely differ.

use std::path::Path;
use std::process::Command;
use x_core::error::{Error, Result};
use x_core::file::FileManager;

/// The platform file adapter.
pub struct PlatformFile;

#[cfg(unix)]
fn username_for_uid(uid: u32) -> Option<String> {
    name_for_field("/etc/passwd", 2, &uid.to_string())
}

#[cfg(unix)]
fn groupname_for_gid(gid: u32) -> Option<String> {
    name_for_field("/etc/group", 2, &gid.to_string())
}

#[cfg(unix)]
fn info_target_mode(path: &Path) -> u32 {
    std::fs::symlink_metadata(path)
        .map(|m| {
            use std::os::unix::fs::MetadataExt;
            m.mode() & 0o777
        })
        .unwrap_or(0)
}

/// In a colon separated database, find the line whose field `key_index`
/// equals `key` and return field `name_index`.
#[cfg(unix)]
fn name_for_field(database: &str, key_index: usize, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(database).ok()?;
    text.lines().find_map(|line| {
        let fields: Vec<&str> = line.split(':').collect();
        (fields.get(key_index) == Some(&key))
            .then(|| fields.first().map(|n| n.to_string()))
            .flatten()
    })
}

impl FileManager for PlatformFile {
    fn info(&self, path: &Path) -> Result<x_core::file::FileInfo> {
        let mut info = x_core::file::file_info(
            path,
            &std::fs::symlink_metadata(path)
                .map_err(|e| Error::not_found(format!("{}: {e}", path.display())))?,
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if let Ok(meta) = std::fs::metadata(path) {
                info.mode = Some(format!("{:o}", meta.mode() & 0o777));
                info.owner = username_for_uid(meta.uid());
                info.group = groupname_for_gid(meta.gid());
            } else {
                info.mode = Some(format!("{:o}", info_target_mode(path)));
            }
        }
        #[cfg(windows)]
        {
            // Ownership via icacls is one process per call; skip unless asked.
            info.owner = None;
        }
        Ok(info)
    }

    fn acl(&self, path: &Path) -> Result<Option<String>> {
        #[cfg(windows)]
        {
            let native = path.to_string_lossy().replace('/', "\\");
            let output = Command::new("icacls")
                .arg(&native)
                .output()
                .map_err(|e| Error::system(format!("cannot run icacls: {e}")))?;
            if output.status.success() {
                return Ok(Some(
                    String::from_utf8_lossy(&output.stdout).trim().to_string(),
                ));
            }
            Ok(None)
        }
        #[cfg(unix)]
        {
            // POSIX ACL detail needs `getfacl`; absent → basic mode only.
            if let Ok(output) = Command::new("getfacl").arg("-p").arg(path).output() {
                if output.status.success() {
                    return Ok(Some(
                        String::from_utf8_lossy(&output.stdout).trim().to_string(),
                    ));
                }
            }
            Ok(None)
        }
        #[cfg(not(any(unix, windows)))]
        return Ok(None);
    }

    fn open(&self, path: &Path) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            run("open", &[&path.to_string_lossy()])
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            run("xdg-open", &[&path.to_string_lossy()])
        }
        #[cfg(windows)]
        {
            // `explorer <path>` opens the file or directory with the default
            // association; it always exits 1 even on success, so ignore status.
            let _ = Command::new("explorer")
                .arg(path)
                .spawn()
                .map_err(|e| Error::system(format!("cannot open {}: {e}", path.display())))?;
            Ok(())
        }
        #[cfg(not(any(unix, windows)))]
        return Err(Error::unsupported("open is not supported on this platform"));
    }

    fn reveal(&self, path: &Path) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            run("open", &["-R", &path.to_string_lossy()])
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            // No "select in file manager" verb that works across desktops;
            // opening the parent directory is the honest equivalent.
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            run("xdg-open", &[&parent.to_string_lossy()])
        }
        #[cfg(windows)]
        {
            // `/select,` expects a backslash separated argument and exits 1 on
            // success as well.
            let native = path.to_string_lossy().replace('/', "\\");
            let _ = Command::new("explorer")
                .arg(format!("/select,{native}"))
                .spawn()
                .map_err(|e| Error::system(format!("cannot reveal {}: {e}", path.display())))?;
            Ok(())
        }
        #[cfg(not(any(unix, windows)))]
        return Err(Error::unsupported(
            "reveal is not supported on this platform",
        ));
    }

    fn trash(&self, path: &Path) -> Result<()> {
        if !path.exists() && std::fs::symlink_metadata(path).is_err() {
            return Err(Error::not_found(format!(
                "{}: no such file",
                path.display()
            )));
        }
        #[cfg(target_os = "macos")]
        {
            // AppleScript is the only sanctioned "move to Trash" verb; Finder
            // handles name collisions and Same Volume rules itself.
            let script = format!(
                "tell application \"Finder\" to delete POSIX file \"{}\"",
                path.canonicalize()
                    .unwrap_or_else(|_| path.to_path_buf())
                    .display()
            );
            run("osascript", &["-e", &script])
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            // XDG trash first (freedesktop spec), then gio, then a plain
            // move to ~/.local/share/Trash/files as a last resort.
            if x_core::devenv::which("gio").is_some() {
                return run("gio", &["trash", &path.to_string_lossy()]);
            }
            let home = std::env::var("HOME").unwrap_or_default();
            if home.is_empty() {
                return Err(Error::unsupported(
                    "no trash tool available (install gvfs for `gio trash`)",
                ));
            }
            let files = Path::new(&home).join(".local/share/Trash/files");
            std::fs::create_dir_all(&files)
                .map_err(|e| Error::system(format!("cannot create trash dir: {e}")))?;
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "item".to_string());
            let mut target = files.join(&name);
            let mut n = 1;
            while target.exists() {
                target = files.join(format!("{name}.{n}"));
                n += 1;
            }
            std::fs::rename(path, &target)
                .map_err(|e| Error::system(format!("cannot trash {}: {e}", path.display())))
        }
        #[cfg(windows)]
        {
            // PowerShell's VisualBasic FileSystem API deletes to the Recycle
            // Bin, honouring name collisions, without extra dependencies.
            let native = path.to_string_lossy().replace('/', "\\");
            let script = format!(
                "Add-Type -AssemblyName Microsoft.VisualBasic; \
                 [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteFile('{native}', \
                 'OnlyErrorDialogs', 'SendToRecycleBin')"
            );
            let output = Command::new("powershell")
                .args(["-NoProfile", "-Command", &script])
                .output()
                .map_err(|e| Error::system(format!("cannot run powershell: {e}")))?;
            if output.status.success() {
                return Ok(());
            }
            // Directories need the directory overload.
            let dir_script = script.replace("DeleteFile", "DeleteDirectory");
            let output = Command::new("powershell")
                .args(["-NoProfile", "-Command", &dir_script])
                .output()
                .map_err(|e| Error::system(format!("cannot run powershell: {e}")))?;
            if output.status.success() {
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(Error::system(format!(
                    "cannot trash {}: {}",
                    path.display(),
                    stderr.trim()
                )))
            }
        }
        #[cfg(not(any(unix, windows)))]
        return Err(Error::unsupported(
            "trash is not supported on this platform",
        ));
    }
}

#[cfg(unix)]
fn run(program: &str, args: &[&str]) -> Result<()> {
    let _ = Command::new(program)
        .args(args)
        .status()
        .map_err(|e| Error::system(format!("cannot run {program}: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_on_a_missing_path_is_not_found() {
        let err = PlatformFile
            .info(Path::new("x-definitely-missing-file-aa"))
            .unwrap_err();
        assert_eq!(err.kind(), x_core::error::ErrorKind::NotFound);
    }
}
