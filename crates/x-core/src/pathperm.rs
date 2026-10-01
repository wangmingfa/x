//! Path permission checks.
//!
//! The *check* half is pure `std` — it answers "can I read / write" by trying,
//! which is the only cross-platform truth. "Can I execute" is OS knowledge
//! (Unix x bit vs. Windows PATHEXT), so the caller hands in a probe from
//! `x-platform::common::pathperm_os`. The *detail* half (mode bits, owner,
//! ACLs) comes from the file capability, whose platform adapter knows
//! `icacls` and `getfacl`.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// What the current process may do with a path.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AccessCheck {
    /// The path exists.
    pub exists: bool,
    /// Readable by this process.
    pub readable: bool,
    /// Writable by this process.
    pub writable: bool,
    /// Executable / searchable by this process.
    pub executable: bool,
}

/// Probe a path with real opens: the same check the OS itself applies.
/// `is_executable` answers the one question `std` does not share across OSes.
pub fn check(path: &Path, is_executable: fn(&Path) -> bool) -> AccessCheck {
    let metadata = std::fs::symlink_metadata(path);
    let exists = metadata.is_ok();
    let is_dir = metadata.as_ref().is_ok_and(|m| m.is_dir());

    let readable = exists && std::fs::read_dir(path).is_ok()
        || (!is_dir && exists && std::fs::File::open(path).is_ok());
    let writable = exists
        && if is_dir {
            // Opening a directory for write is impossible; creation is the test.
            std::env::temp_dir().exists() && create_probe(path).is_ok()
        } else {
            std::fs::OpenOptions::new().append(true).open(path).is_ok()
        };
    let executable = exists
        && if is_dir {
            std::fs::read_dir(path).is_ok()
        } else {
            is_executable(path)
        };

    AccessCheck {
        exists,
        readable,
        writable,
        executable,
    }
}

/// Creation probe inside a directory: does a scratch file come and go cleanly.
fn create_probe(dir: &Path) -> std::io::Result<()> {
    let probe = dir.join(format!(".x-perm-probe-{}", std::process::id()));
    let result = std::fs::File::create(&probe);
    let _ = std::fs::remove_file(&probe);
    result.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("x-core-perm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn readable_writable_file_round_trips() {
        let dir = temp_dir();
        let file = dir.join("a.txt");
        std::fs::write(&file, b"data").unwrap();

        let access = check(&file, |_| false);
        assert!(access.exists);
        assert!(access.readable);
        assert!(access.writable);
        assert!(!access.executable, "the probe is the answer");

        let executable = check(&file, |_| true);
        assert!(executable.executable);

        let missing = check(&dir.join("no-such-file-aa"), |_| true);
        assert!(!missing.exists);
        assert!(!missing.readable);
        assert!(!missing.executable, "a missing path executes nothing");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
