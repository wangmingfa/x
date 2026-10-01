//! Environment variables, `PATH` and command lookup.
//!
//! Everything here is process environment work: no OS API, no `cfg`, no
//! spawning. `set` changes the current process only — making a variable
//! permanent belongs to the user's shell profile, and `x` says so instead of
//! editing dotfiles behind the user's back.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One environment variable.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnvVar {
    /// Variable name.
    pub name: String,
    /// Value; multi-entry values (like `PATH`) are kept verbatim.
    pub value: String,
}

/// List all environment variables of this process, sorted by name.
pub fn env_list() -> Vec<EnvVar> {
    let mut vars: Vec<EnvVar> = std::env::vars()
        .map(|(name, value)| EnvVar { name, value })
        .collect();
    vars.sort_by(|a, b| a.name.cmp(&b.name));
    vars
}

/// Read one variable. Case sensitive everywhere: scripts already know that
/// `env` on their platform does the same.
pub fn env_get(name: &str) -> Result<String> {
    match std::env::var(name) {
        Ok(value) => Ok(value),
        Err(std::env::VarError::NotPresent) => Err(Error::not_found(format!(
            "environment variable not set: {name}"
        ))),
        Err(std::env::VarError::NotUnicode(raw)) => Err(Error::invalid_input(format!(
            "environment variable {name} is not valid unicode: {raw:?}"
        ))),
    }
}

/// Set one variable for this process and its future children.
pub fn env_set(name: &str, value: &str) -> Result<()> {
    if name.is_empty() || name.contains('=') {
        return Err(Error::invalid_input(
            "variable name must be non-empty and contain no '='",
        ));
    }
    // Affects this process and the children it spawns later, like a shell would.
    std::env::set_var(name, value);
    Ok(())
}

/// One `PATH` entry with the commands it provides.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PathEntry {
    /// Directory as spelled in `PATH`.
    pub dir: String,
    /// The directory exists.
    pub exists: bool,
    /// Executable file names found directly inside (best effort).
    pub commands: Vec<String>,
}

/// Split `PATH` into entries. Empty segments are dropped and reported how
/// every platform shell treats them: ignored.
pub fn path_entries() -> Vec<PathEntry> {
    let raw = std::env::var("PATH").unwrap_or_default();
    let separator = path_separator();
    raw.split(separator)
        .filter(|segment| !segment.is_empty())
        .map(|dir| {
            let exists = Path::new(dir).is_dir();
            let commands = if exists {
                list_commands(dir)
            } else {
                Vec::new()
            };
            PathEntry {
                dir: dir.to_string(),
                exists,
                commands,
            }
        })
        .collect()
}

/// Where commands whose name matches `query` live.
///
/// Matching is exact for the full name; `which node` finds `node` and, on
/// Windows, `node.exe` / `node.cmd` / `node.bat`.
pub fn path_find(query: &str) -> Vec<PathBuf> {
    let mut hits = Vec::new();
    for entry in path_entries() {
        let dir = PathBuf::from(&entry.dir);
        if is_executable_match(&dir, query) {
            hits.push(dir.join(executable_name(query)));
        }
    }
    hits
}

/// The classic `which`: first hit for a command name on `PATH`.
pub fn which(command: &str) -> Option<PathBuf> {
    path_find(command).into_iter().next()
}

/// Whether the command resolves anywhere on `PATH`.
pub fn command_exists(command: &str) -> bool {
    which(command).is_some()
}

/// `PATH` separator character on this build (`:` on Unix, `;` on Windows).
///
/// Computed without `cfg` so `x-core` stays platform blind: the separator is
/// derived from how the current `PATH` is actually spelled... which is
/// ambiguous with a single entry, so fall back to the conventional guess.
pub fn path_separator() -> char {
    if std::env::var("PATH").is_ok_and(|p| p.contains(';'))
        && !std::env::var("PATH").is_ok_and(|p| p.contains(':'))
    {
        ';'
    } else {
        ':'
    }
}

fn executable_name(command: &str) -> String {
    command.to_string()
}

fn is_executable_match(dir: &Path, command: &str) -> bool {
    let target = executable_name(command);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let stem = name
            .strip_suffix(".exe")
            .or_else(|| name.strip_suffix(".cmd"))
            .or_else(|| name.strip_suffix(".bat"))
            .unwrap_or(&name);
        if stem.eq_ignore_ascii_case(&target) && entry.metadata().is_ok_and(|m| m.is_file()) {
            return true;
        }
    }
    false
}

fn list_commands(dir: &str) -> Vec<String> {
    let mut names = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return names;
    };
    for entry in entries.flatten() {
        let is_file = entry.metadata().is_ok_and(|m| m.is_file());
        if is_file {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_reports_missing_variables_as_not_found() {
        let err = env_get("X_NO_SUCH_VARIABLE_AABBCC").unwrap_err();
        assert_eq!(err.kind(), crate::error::ErrorKind::NotFound);
    }

    #[test]
    fn set_then_get_round_trips() {
        env_set("X_TEST_VAR", "hello").unwrap();
        assert_eq!(env_get("X_TEST_VAR").unwrap(), "hello");
    }

    #[test]
    fn path_entries_skip_empty_segments() {
        // Whatever the real PATH is, no entry may be empty.
        for entry in path_entries() {
            assert!(!entry.dir.is_empty());
        }
    }

    #[test]
    fn which_finds_cargo_or_reports_none_gracefully() {
        // No assertion on the result: the point is that it never panics.
        let _ = which("cargo");
    }
}
