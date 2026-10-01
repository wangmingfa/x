//! Unified file operations.
//!
//! Inspection (`info`, `size`, `copy`, `move`, `rename`) is plain `std::fs`
//! and therefore implemented once here in `x-core`. The three verbs whose
//! behaviour genuinely differs per OS — `open` (double click), `reveal`
//! (select in the file manager) and `trash` (move to the recycle bin) — are
//! the required methods each platform adapter must provide.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// What kind of entry a path refers to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileType {
    /// Regular file.
    #[default]
    File,
    /// Directory.
    Dir,
    /// Symbolic link (reported before the target is resolved).
    Symlink,
    /// Anything else (device, fifo, socket, ...).
    Other,
}

impl FileType {
    /// Lowercase identifier.
    pub const fn name(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Dir => "dir",
            Self::Symlink => "symlink",
            Self::Other => "other",
        }
    }
}

/// Everything `x file info` reports about one path.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FileInfo {
    /// Path as requested (not canonicalized).
    pub path: PathBuf,
    /// Entry kind.
    pub file_type: FileType,
    /// Size in bytes; `None` when the platform cannot tell (e.g. some specials).
    pub size: Option<u64>,
    /// Unix mode in octal (e.g. `644`), `None` on Windows.
    pub mode: Option<String>,
    /// Whether the current process may write to the entry.
    pub readonly: bool,
    /// Owner user name, when the platform exposes one.
    pub owner: Option<String>,
    /// Owning group name, when the platform exposes one.
    pub group: Option<String>,
    /// Last modification time, RFC 3339.
    pub modified: Option<String>,
    /// Creation ("birth") time, RFC 3339, when the filesystem records it.
    pub created: Option<String>,
    /// Symbolic link target for `FileType::Symlink`.
    pub target: Option<PathBuf>,
}

impl FileInfo {
    /// Human readable type label used by tables.
    pub fn type_label(&self) -> &'static str {
        match self.file_type {
            FileType::File => "file",
            FileType::Dir => "directory",
            FileType::Symlink => "symlink",
            FileType::Other => "special",
        }
    }
}

/// One entry of a directory listing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DirEntryInfo {
    /// File name (no parent path).
    pub name: String,
    /// Full path.
    pub path: PathBuf,
    /// Entry kind.
    pub file_type: FileType,
    /// Size in bytes (files only; directories report `None`).
    pub size: Option<u64>,
}

/// File inspection and the platform specific "open" family.
pub trait FileManager: Send + Sync {
    /// Metadata for one path.
    fn info(&self, path: &Path) -> Result<FileInfo> {
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|e| Error::not_found(format!("{}: {e}", path.display())))?;
        Ok(file_info(path, &metadata))
    }

    /// Recursive byte count. Directories are walked; unreadable entries are
    /// skipped rather than failing the whole command.
    fn size(&self, path: &Path) -> Result<u64> {
        let metadata = std::fs::symlink_metadata(path)
            .map_err(|e| Error::not_found(format!("{}: {e}", path.display())))?;
        Ok(recursive_size(path, &metadata))
    }

    /// Names inside a directory, sorted, hidden entries included.
    fn list(&self, path: &Path) -> Result<Vec<DirEntryInfo>> {
        let mut rows = Vec::new();
        let entries = std::fs::read_dir(path)
            .map_err(|e| Error::not_found(format!("{}: {e}", path.display())))?;
        for entry in entries.flatten() {
            let file_type = match entry.file_type() {
                Ok(ft) if ft.is_symlink() => FileType::Symlink,
                Ok(ft) if ft.is_dir() => FileType::Dir,
                Ok(ft) if ft.is_file() => FileType::File,
                _ => FileType::Other,
            };
            let size = entry
                .metadata()
                .ok()
                .filter(|_| file_type == FileType::File)
                .map(|m| m.len());
            rows.push(DirEntryInfo {
                name: entry.file_name().to_string_lossy().into_owned(),
                path: entry.path(),
                file_type,
                size,
            });
        }
        rows.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(rows)
    }

    /// Open the path the way a double click would (default application).
    fn open(&self, path: &Path) -> Result<()>;

    /// Show the path in the platform file manager, selecting it when the
    /// platform supports that (macOS Finder, Windows Explorer).
    fn reveal(&self, path: &Path) -> Result<()>;

    /// Move the path to the platform trash / recycle bin.
    fn trash(&self, path: &Path) -> Result<()>;

    /// Platform-native access control detail (Windows ACL, POSIX ACL), when
    /// the platform can produce one cheaply. `None` means "only the basic
    /// mode bits apply" or "no acl tool present".
    fn acl(&self, _path: &Path) -> Result<Option<String>> {
        Ok(None)
    }

    /// Copy a file or a directory tree. Existing destinations are overwritten.
    fn copy(&self, from: &Path, to: &Path) -> Result<()> {
        copy_path(from, to)
    }

    /// Move (or rename) a file or directory.
    fn move_path(&self, from: &Path, to: &Path) -> Result<()> {
        move_path(from, to)
    }

    /// Rename inside the same directory; convenience over [`Self::move_path`].
    fn rename(&self, path: &Path, new_name: &str) -> Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| Error::invalid_input(format!("{} has no parent", path.display())))?;
        if new_name.contains(['/', '\\']) {
            return Err(Error::invalid_input(
                "new name must not contain a path separator",
            ));
        }
        move_path(path, &parent.join(new_name))
    }
}

/// Build a [`FileInfo`] from already fetched metadata.
///
/// Public so platform adapters that need richer owner information can extend
/// the result instead of re-deriving it.
pub fn file_info(path: &Path, metadata: &std::fs::Metadata) -> FileInfo {
    let file_type = if metadata.file_type().is_symlink() {
        FileType::Symlink
    } else if metadata.is_dir() {
        FileType::Dir
    } else if metadata.is_file() {
        FileType::File
    } else {
        FileType::Other
    };
    FileInfo {
        path: path.to_path_buf(),
        file_type,
        size: metadata.is_file().then_some(metadata.len()),
        mode: None,
        readonly: metadata.permissions().readonly(),
        owner: None,
        group: None,
        modified: system_time_to_rfc3339(metadata.modified().ok()),
        created: system_time_to_rfc3339(metadata.created().ok()),
        target: std::fs::read_link(path).ok(),
    }
}

/// System time → RFC 3339, `None` when the clock is before 1970.
pub fn system_time_to_rfc3339(time: Option<std::time::SystemTime>) -> Option<String> {
    let seconds = time?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
    Some(crate::audit::rfc3339_utc(seconds))
}

fn recursive_size(path: &Path, metadata: &std::fs::Metadata) -> u64 {
    if !metadata.is_dir() {
        return metadata.len();
    }
    let mut total = 0;
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        total += if meta.is_dir() {
            recursive_size(&entry.path(), &meta)
        } else {
            meta.len()
        };
    }
    total
}

fn copy_path(from: &Path, to: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(from)
        .map_err(|e| Error::not_found(format!("{}: {e}", from.display())))?;
    if meta.is_dir() {
        std::fs::create_dir_all(to)
            .map_err(|e| Error::system(format!("cannot create {}: {e}", to.display())))?;
        let entries = std::fs::read_dir(from)
            .map_err(|e| Error::system(format!("cannot read {}: {e}", from.display())))?;
        for entry in entries.flatten() {
            copy_path(&entry.path(), &to.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(from, to).map(|_| ()).map_err(|e| {
            Error::system(format!(
                "cannot copy {} -> {}: {e}",
                from.display(),
                to.display()
            ))
        })
    }
}

fn move_path(from: &Path, to: &Path) -> Result<()> {
    if let Err(rename_error) = std::fs::rename(from, to) {
        // Cross-device moves need a copy + delete.
        copy_path(from, to)?;
        remove_path(from).map_err(|_e| {
            Error::system(format!(
                "copied to {} but cannot remove source {}: {rename_error}",
                to.display(),
                from.display()
            ))
        })?;
    }
    Ok(())
}

fn remove_path(path: &Path) -> Result<()> {
    let meta = std::fs::symlink_metadata(path)
        .map_err(|e| Error::not_found(format!("{}: {e}", path.display())))?;
    if meta.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
    .map_err(|e| Error::system(format!("cannot remove {}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("x-core-file-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn copy_and_move_a_tree() {
        let dir = temp_dir("copy-move");
        let src = dir.join("src");
        std::fs::create_dir_all(src.join("nested")).unwrap();
        std::fs::write(src.join("nested/a.txt"), b"hello").unwrap();

        let copied = dir.join("dst");
        copy_path(&src, &copied).unwrap();
        assert_eq!(
            std::fs::read(copied.join("nested/a.txt")).unwrap(),
            b"hello"
        );

        move_path(&copied, &dir.join("renamed")).unwrap();
        assert!(!copied.exists());
        assert!(dir.join("renamed/nested/a.txt").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn size_walks_directories() {
        let dir = temp_dir("size");
        std::fs::write(dir.join("a"), vec![0u8; 100]).unwrap();
        std::fs::create_dir(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/b"), vec![0u8; 30]).unwrap();
        assert_eq!(recursive_size(&dir, &std::fs::metadata(&dir).unwrap()), 130);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
