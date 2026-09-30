//! Helpers shared by the platform adapters.

/// `true` on macOS (including iOS targets that compile the same sources).
pub const fn is_macos() -> bool {
    cfg!(target_os = "macos")
}

/// `true` on Linux.
pub const fn is_linux() -> bool {
    cfg!(target_os = "linux")
}

/// `true` on Windows.
pub const fn is_windows() -> bool {
    cfg!(target_os = "windows")
}

/// Name of the current OS user, empty when unknown.
pub fn current_user_name() -> Option<String> {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .ok()
        .filter(|u| !u.is_empty())
}

/// Name of the current host.
pub fn host_name() -> Option<String> {
    if let Ok(name) = std::env::var("HOSTNAME") {
        if !name.is_empty() {
            return Some(name);
        }
    }
    sysinfo::System::host_name()
}

/// Run a command and return its stdout, used only where the platform has no
/// native API for the job (Level 3 of the native-first policy).
///
/// `shell: false` by default so arguments are never re-parsed by a shell.
pub fn run_command(program: &str, args: &[&str]) -> std::io::Result<String> {
    let output = std::process::Command::new(program).args(args).output()?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "{program} exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `true` when an executable is reachable through `PATH` or an absolute path.
pub fn command_exists(program: &str) -> bool {
    if program.contains('/') {
        return std::path::Path::new(program).exists();
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join(program).exists())
}

/// Truncate a string to `width` columns, adding an ellipsis when cut.
pub fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out: String = text.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Pad `text` to `width` columns, truncating when needed.
pub fn pad(text: &str, width: usize) -> String {
    let mut out = truncate(text, width);
    let len = out.chars().count();
    out.extend(std::iter::repeat_n(' ', width.saturating_sub(len)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_and_pad() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 8), "hello w…");
        assert_eq!(truncate("hello", 0), "");
        assert_eq!(pad("ab", 5), "ab   ");
        assert_eq!(pad("abcdef", 3), "ab…");
    }

    #[test]
    fn command_detection() {
        assert!(command_exists("sh"));
        assert!(!command_exists("definitely-not-a-real-binary-xyz"));
    }

    #[test]
    fn current_user_is_present_on_unix() {
        if cfg!(windows) {
            return;
        }
        assert!(current_user_name().is_some());
    }
}
