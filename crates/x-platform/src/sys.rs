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

/// Run a command and keep its exit code and both streams, for the native
/// escape hatches: the child's failure is data to show, not an error to raise.
///
/// Only a spawn failure is an `io::Error`; a non-zero exit is reported as the
/// child's own answer. Streams stay raw bytes because Windows console programs
/// speak the OEM code page while everything else is UTF-8.
pub fn run_command_capture(
    program: &str,
    args: &[&str],
) -> std::io::Result<(i32, Vec<u8>, Vec<u8>)> {
    let output = std::process::Command::new(program).args(args).output()?;
    Ok((
        output.status.code().unwrap_or(-1),
        output.stdout,
        output.stderr,
    ))
}

/// Lossy UTF-8 decoding, the correct answer everywhere except the Windows
/// console programs covered by the OEM code page.
pub fn decode_bytes_lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Run one service manager's own command line and package the raw answer.
///
/// A non-zero exit is part of the answer; only a failed spawn is an error.
pub fn run_native_lossy(
    program: &str,
    args: &[String],
) -> std::io::Result<x_core::service::NativeOutput> {
    let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
    let (exit_code, stdout, stderr) = run_command_capture(program, &borrowed)?;
    Ok(x_core::service::NativeOutput {
        program: program.to_string(),
        args: args.to_vec(),
        exit_code,
        stdout: decode_bytes_lossy(&stdout),
        stderr: decode_bytes_lossy(&stderr),
    })
}

/// `true` when an executable is reachable through `PATH` or an absolute path.
pub fn command_exists(program: &str) -> bool {
    if program.contains('/') || program.contains('\\') {
        return std::path::Path::new(program).exists();
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(program);
        // Windows resolves an extensionless spawn by appending `.exe`
        // (CreateProcess behavior), so the check must do the same.
        candidate.exists() || candidate.with_extension("exe").exists()
    })
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
        // A shell guaranteed to exist on each platform, nothing else.
        let known = if cfg!(windows) { "cmd" } else { "sh" };
        assert!(command_exists(known));
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
