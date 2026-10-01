//! Clipboard through each platform's stock command line tools.
//!
//! macOS: `pbcopy` / `pbpaste`. Linux: `wl-copy` / `wl-paste` on Wayland,
//! `xclip` on X11. Windows: PowerShell clipboard cmdlets.

use std::process::{Command, Stdio};
use x_core::clipboard::ClipboardManager;
use x_core::error::{Error, Result};

/// The platform clipboard adapter.
pub struct PlatformClipboard;

impl ClipboardManager for PlatformClipboard {
    fn get(&self) -> Result<String> {
        #[cfg(target_os = "macos")]
        return read("pbpaste", &[]);
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            if x_core::devenv::which("wl-paste").is_some() {
                return read("wl-paste", &["--no-newline"]);
            }
            if x_core::devenv::which("xclip").is_some() {
                return read("xclip", &["-selection", "clipboard", "-o"]);
            }
            Err(Error::unsupported(
                "no clipboard tool found (install wl-clipboard or xclip)",
            ))
        }
        #[cfg(windows)]
        return read(
            "powershell",
            &["-NoProfile", "-Command", "Get-Clipboard -Raw"],
        );
        #[cfg(not(any(unix, windows)))]
        return Err(Error::unsupported("clipboard is not supported here"));
    }

    fn set(&self, text: &str) -> Result<()> {
        #[cfg(target_os = "macos")]
        return write("pbcopy", &[], text);
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            if x_core::devenv::which("wl-copy").is_some() {
                return write("wl-copy", &[], text);
            }
            if x_core::devenv::which("xclip").is_some() {
                return write("xclip", &["-selection", "clipboard", "-in"], text);
            }
            Err(Error::unsupported(
                "no clipboard tool found (install wl-clipboard or xclip)",
            ))
        }
        #[cfg(windows)]
        return write(
            "powershell",
            &["-NoProfile", "-Command", "$input | Set-Clipboard"],
            text,
        );
        #[cfg(not(any(unix, windows)))]
        return Err(Error::unsupported("clipboard is not supported here"));
    }

    fn clear(&self) -> Result<()> {
        self.set("")
    }
}

fn read(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .map_err(|e| Error::unsupported(format!("{program} is not available: {e}")))?;
    if !output.status.success() {
        return Err(Error::system(format!(
            "{program} failed (exit {}); clipboard may be empty",
            output.status.code().unwrap_or(-1)
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn write(program: &str, args: &[&str], text: &str) -> Result<()> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| Error::unsupported(format!("{program} is not available: {e}")))?;
    if let Some(stdin) = child.stdin.as_mut() {
        use std::io::Write;
        stdin
            .write_all(text.as_bytes())
            .map_err(|e| Error::system(format!("cannot write to {program}: {e}")))?;
    }
    drop(child.stdin.take());
    let status = child
        .wait()
        .map_err(|e| Error::system(format!("{program} failed: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::system(format!(
            "{program} exited with {}",
            status.code().unwrap_or(-1)
        )))
    }
}
