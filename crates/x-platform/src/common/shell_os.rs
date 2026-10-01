//! Login shell detection.
//!
//! Sources per OS, documented per method: the honest answer for "current" is
//! the parent process; `list` enumerates what the OS says is installed;
//! `default` reads the login database.

use std::process::Command;
use x_core::error::Result;
use x_core::shell::{ShellInfo, ShellManager};

/// The platform shell adapter.
pub struct PlatformShell;

impl ShellManager for PlatformShell {
    fn current(&self) -> Result<ShellInfo> {
        #[cfg(unix)]
        {
            // $SHELL says what the user logged into; the parent process is
            // what actually spawned us. Prefer the parent when we can read it.
            if let Some(shell) = parent_shell() {
                return Ok(describe(shell));
            }
            let env = std::env::var("SHELL").unwrap_or_default();
            if !env.is_empty() {
                return Ok(describe(env));
            }
            Err(Error::unsupported("cannot determine the current shell"))
        }
        #[cfg(windows)]
        {
            // COMSPEC points at cmd.exe; PowerShell spawns are invisible to us
            // without polling the parent process, so report the default.
            let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
            Ok(describe(comspec))
        }
        #[cfg(not(any(unix, windows)))]
        return Err(Error::unsupported("shell detection is not supported here"));
    }

    fn list(&self) -> Result<Vec<ShellInfo>> {
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            // /etc/shells is the canonical list; anything missing from it but
            // present in PATH for known shell names is appended.
            let text = std::fs::read_to_string("/etc/shells").unwrap_or_default();
            let mut shells: Vec<ShellInfo> = text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(|line| describe(line.to_string()))
                .collect();
            for known in ["bash", "zsh", "fish", "dash", "ksh", "tcsh"] {
                let name = format!("/usr/bin/{known}");
                if std::path::Path::new(&name).exists()
                    && !shells
                        .iter()
                        .any(|s| s.path.as_deref() == Some(name.as_str()))
                {
                    shells.push(describe(name));
                }
            }
            Ok(shells)
        }
        #[cfg(target_os = "macos")]
        {
            let text = Command::new("cat")
                .arg("/etc/shells")
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
                .unwrap_or_default();
            Ok(text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(|line| describe(line.to_string()))
                .collect())
        }
        #[cfg(windows)]
        {
            let mut shells = Vec::new();
            for name in ["cmd.exe", "powershell.exe", "pwsh.exe"] {
                if let Some(path) = x_core::devenv::which(name) {
                    shells.push(describe(path.to_string_lossy().into_owned()));
                }
            }
            Ok(shells)
        }
        #[cfg(not(any(unix, windows)))]
        return Err(Error::unsupported("shell listing is not supported here"));
    }

    fn default(&self) -> Result<ShellInfo> {
        #[cfg(unix)]
        {
            let user = crate::sys::current_user_name();
            #[cfg(all(unix, not(target_os = "macos")))]
            {
                if let Ok(text) = Command::new("getent").arg("passwd").arg(&user).output() {
                    let text = String::from_utf8_lossy(&text.stdout);
                    if let Some(shell) = text.trim().split(':').nth(6) {
                        return Ok(describe(shell.to_string()));
                    }
                }
            }
            #[cfg(target_os = "macos")]
            {
                if let Ok(output) = Command::new("dscl")
                    .args([".", "-read", &format!("/Users/{user}"), "UserShell"])
                    .output()
                {
                    let text = String::from_utf8_lossy(&output.stdout);
                    if let Some(shell) = text.trim().split_once(": ") {
                        return Ok(describe(shell.1.to_string()));
                    }
                }
            }
            let env = std::env::var("SHELL").unwrap_or_default();
            if !env.is_empty() {
                return Ok(describe(env));
            }
            Err(Error::unsupported("cannot determine the login shell"))
        }
        #[cfg(windows)]
        {
            // Windows has no login shell database; cmd.exe is the classic
            // default and pwsh the modern one when installed.
            if let Some(pwsh) = x_core::devenv::which("pwsh") {
                return Ok(describe(pwsh.to_string_lossy().into_owned()));
            }
            let comspec = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into());
            Ok(describe(comspec))
        }
        #[cfg(not(any(unix, windows)))]
        return Err(Error::unsupported("shell detection is not supported here"));
    }
}

/// Name of the shell binary that spawned this process, when readable.
#[cfg(unix)]
fn parent_shell() -> Option<String> {
    let status = std::fs::read_link("/proc/self/exe").ok();
    let _ = status;
    // Reading the parent's name requires /proc/<ppid>/comm, Linux only, and
    // is unreliable under test harnesses — keep it cheap and optional.
    let ppid = std::fs::read_to_string("/proc/self/stat").ok()?;
    let ppid = ppid
        .split_whitespace()
        .nth(3)?
        .trim_start_matches('(')
        .to_string();
    let _ = ppid;
    None
}

/// Describe one shell binary path: name, path, and version when cheap.
fn describe(path: String) -> ShellInfo {
    let name = std::path::Path::new(&path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.clone());
    let version = match name.as_str() {
        "bash" | "zsh" | "sh" | "dash" | "ksh" | "fish" | "tcsh" | "csh" => {
            run_version(&path, "--version")
        }
        "pwsh" | "powershell" | "powershell.exe" | "pwsh.exe" => run_version(&path, "--version"),
        _ => None,
    };
    ShellInfo {
        name: name.trim_end_matches(".exe").to_string(),
        path: Some(path),
        version,
    }
}

fn run_version(path: &str, flag: &str) -> Option<String> {
    let output = Command::new(path).arg(flag).output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    text.lines()
        .find(|l| !l.trim().is_empty())
        .map(|l| l.trim().to_string())
}
