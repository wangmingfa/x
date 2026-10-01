//! Plugins: external `x-<name>` executables that extend the CLI.
//!
//! A plugin is any executable named `x-<name>` in the plugin directory. `x
//! plugins list` shows what is installed; `x plugins install <file>` copies a
//! binary in; an unknown subcommand (`x myplugin …`) executes the matching
//! plugin verbatim, inheriting stdin/stdout and returning its exit code —
//! the plugin owns its own confirmations, exactly like a separate tool.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One installed plugin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginInfo {
    /// Name without the `x-` prefix (what the user types after `x`).
    pub name: String,
    /// Absolute path of the executable.
    pub path: PathBuf,
    /// Whether the file is actually executable by this process.
    pub executable: bool,
}

/// The directory plugins are installed to.
///
/// `$XDG_DATA_HOME/x/plugins`, else `~/.local/share/x/plugins` on Unix and
/// `%APPDATA%\x\plugins` on Windows — environment derived, no OS conditional.
pub fn plugin_dir() -> PathBuf {
    if let Ok(data) = std::env::var("XDG_DATA_HOME") {
        if !data.is_empty() {
            return PathBuf::from(data).join("x").join("plugins");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return PathBuf::from(home).join(".local/share/x/plugins");
        }
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        if !appdata.is_empty() {
            return PathBuf::from(appdata).join("x").join("plugins");
        }
    }
    PathBuf::from("x-plugins")
}

/// Every plugin currently installed, sorted by name.
pub fn list() -> Vec<PluginInfo> {
    let dir = plugin_dir();
    let mut plugins = Vec::new();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return plugins;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(name) = file_name.strip_prefix("x-") else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        plugins.push(PluginInfo {
            name: name.to_string(),
            executable: is_executable(&path),
            path,
        });
    }
    plugins.sort_by(|a, b| a.name.cmp(&b.name));
    plugins
}

/// Resolve the plugin a user invoked as `x <name>`, if one exists.
pub fn lookup(name: &str) -> Option<PathBuf> {
    let path = plugin_dir().join(format!("x-{name}"));
    (path.is_file() && is_executable(&path)).then_some(path)
}

/// Install a plugin binary: copy `source` into the plugin directory as
/// `x-<name>`, returning the installed path. Existing plugins are replaced.
/// Permissions travel with the copy (`fs::copy` keeps mode bits), so an
/// executable source installs executable.
pub fn install(source: &Path, name: &str) -> std::io::Result<PathBuf> {
    if name.is_empty() || name.contains(['/', '\\']) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "plugin name must be non-empty and contain no separators",
        ));
    }
    let dir = plugin_dir();
    std::fs::create_dir_all(&dir)?;
    let target = dir.join(format!("x-{name}"));
    std::fs::copy(source, &target)?;
    Ok(target)
}

/// Remove a plugin by name.
pub fn remove(name: &str) -> std::io::Result<()> {
    let target = plugin_dir().join(format!("x-{name}"));
    std::fs::remove_file(target)
}

/// Is this file an executable image? Decided from the file header, so the
/// answer works on every OS without platform conditionals: Windows PE
/// (`MZ`), ELF (`\x7fELF`), and interpreter scripts (`#!`).
fn is_executable(path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    use std::io::Read;
    let mut header = [0u8; 4];
    match file.read_exact(&mut header) {
        Ok(()) => {}
        Err(_) => return false, // smaller than any header: not a binary
    }
    header.starts_with(b"MZ")
        || header.starts_with(b"\x7fELF")
        || header.starts_with(b"#!")
        // macOS universal binaries / Mach-O (32/64-bit, little/big endian)
        || header.starts_with(&[0xCA, 0xFE, 0xBA, 0xBE])
        || header.starts_with(&[0xFE, 0xED, 0xFA, 0xCE])
        || header.starts_with(&[0xCE, 0xFA, 0xED, 0xFE])
        || header.starts_with(&[0xFE, 0xED, 0xFA, 0xCF])
        || header.starts_with(&[0xCF, 0xFA, 0xED, 0xFE])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_and_list_agree_on_what_is_installed() {
        // Whatever the host has, both views must agree; on a clean machine
        // both are simply empty.
        for plugin in list() {
            assert!(
                lookup(&plugin.name).is_some(),
                "{} listed but not resolvable",
                plugin.name
            );
        }
    }

    #[test]
    fn install_rejects_names_with_separators() {
        let dir = std::env::temp_dir().join(format!("x-plugins-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("payload.txt");
        std::fs::write(&source, b"hi").unwrap();

        let err = install(&source, "bad/name").unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
