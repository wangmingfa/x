//! Executability: the OS-specific half of a path permission check.
//!
//! `x-core::pathperm` probes read/write by trying; "can execute" is the one
//! answer that is not portable — Unix keeps an x bit, Windows does not.

use std::path::Path;

/// Whether this process could execute `path` in the OS's own sense.
pub fn has_executable_bit(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        // Windows: "executable" is a PATHEXT association, not a bit; report
        // extension-based truth for the common cases.
        path.extension()
            .and_then(|e| e.to_str())
            .map(|e| {
                matches!(
                    e.to_ascii_lowercase().as_str(),
                    "exe" | "bat" | "cmd" | "ps1" | "com"
                )
            })
            .unwrap_or(false)
    }
}
