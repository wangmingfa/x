//! Git through the `git` CLI.
//!
//! x shells out to `git` — the porcelain the user already trusts — and maps
//! its output onto a small model. When `git` is absent every function returns
//! an `Unsupported` error, which `x capability` reports honestly.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// One local or remote branch and where it points.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BranchInfo {
    /// Branch name as `git branch` prints it (`main`, `origin/main`).
    pub name: String,
    /// Short commit hash.
    pub commit: Option<String>,
    /// Whether this is the currently checked out branch.
    pub current: bool,
}

/// `git status --porcelain` line, split the way scripts want it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusEntry {
    /// Two character XY status code (`M `, ` M`, `??`, `UU`).
    pub code: String,
    /// File path relative to the repository root.
    pub path: String,
    /// Second path for renames / copies.
    pub orig_path: Option<String>,
}

/// Summary of `git status`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GitStatus {
    /// Current branch, or `None` on a detached HEAD.
    pub branch: Option<String>,
    /// Working tree changes.
    pub entries: Vec<StatusEntry>,
    /// Unpushed commits (`ahead`), when the branch has an upstream.
    pub ahead: Option<usize>,
    /// Missing upstream commits (`behind`).
    pub behind: Option<usize>,
    /// Whole working tree is clean.
    pub clean: bool,
}

/// Find the nearest enclosing work tree root, like `git rev-parse --show-toplevel`.
pub fn repo_root(start: &Path) -> Result<PathBuf> {
    let output = git(start, ["rev-parse", "--show-toplevel"])?;
    let root = PathBuf::from(output.trim());
    if root.as_os_str().is_empty() {
        return Err(Error::not_found(format!(
            "no git repository at or above {}",
            start.display()
        )));
    }
    Ok(root)
}

/// Short `git status` summary.
pub fn status(start: &Path) -> Result<GitStatus> {
    let branch = git(start, ["rev-parse", "--abbrev-ref", "HEAD"])
        .ok()
        .map(|b| b.trim().to_string())
        .filter(|b| b != "HEAD");

    let porcelain = git(start, ["status", "--porcelain", "-b"])?;
    let mut entries = Vec::new();
    let mut ahead = None;
    let mut behind = None;
    for line in porcelain.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            // `## main...origin/main [ahead 1, behind 2]` or `## No commits yet on main`
            if let Some((_local, upstream)) = rest.split_once("...") {
                let tracking = upstream.split([' ', '[']).next().unwrap_or(upstream);
                let _ = tracking;
                if let Some(counts) = upstream.split('[').nth(1) {
                    for part in counts.trim_end_matches(']').split(',') {
                        let mut kv = part.split_whitespace();
                        match (kv.next(), kv.next().and_then(|n| n.parse().ok())) {
                            (Some("ahead"), Some(n)) => ahead = Some(n),
                            (Some("behind"), Some(n)) => behind = Some(n),
                            _ => {}
                        }
                    }
                }
            }
            continue;
        }
        if line.len() < 4 {
            continue;
        }
        let (code, rest) = line.split_at(2);
        let rest = rest.strip_prefix(' ').unwrap_or(rest);
        let (path, orig_path) = match rest.split_once(" -> ") {
            Some((from, to)) => (to.to_string(), Some(from.to_string())),
            None => (rest.to_string(), None),
        };
        entries.push(StatusEntry {
            code: code.to_string(),
            path,
            orig_path,
        });
    }

    let clean = entries.iter().all(|e| e.code != " M" && e.code != "??") && entries.is_empty();
    Ok(GitStatus {
        branch,
        entries,
        ahead,
        behind,
        clean,
    })
}

/// Local branches (`git branch --no-column`); `remotes` adds `-r`.
pub fn branches(start: &Path, remotes: bool) -> Result<Vec<BranchInfo>> {
    let mut args = vec![
        "branch",
        "--no-column",
        "--format=%(refname:short)%09%(objectname:short)",
    ];
    if remotes {
        args.push("-r");
    }
    let output = git(start, args)?;
    let head = git(start, ["rev-parse", "--abbrev-ref", "HEAD"])
        .ok()
        .map(|b| b.trim().to_string());
    Ok(output
        .lines()
        .filter_map(|line| {
            let (name, commit) = line.split_once('\t')?;
            Some(BranchInfo {
                name: name.trim().trim_matches('*').trim().to_string(),
                commit: Some(commit.trim().to_string()),
                current: head.as_deref() == Some(name.trim().trim_matches('*').trim()),
            })
        })
        .collect())
}

/// Files that changed between the working tree and HEAD, grouped.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChangedFiles {
    /// Modified tracked files.
    pub modified: Vec<String>,
    /// Untracked files.
    pub untracked: Vec<String>,
    /// Deleted tracked files.
    pub deleted: Vec<String>,
}

/// What changed in the working tree.
pub fn changed(start: &Path) -> Result<ChangedFiles> {
    let st = status(start)?;
    let mut files = ChangedFiles::default();
    for entry in st.entries {
        match entry.code.trim() {
            "??" => files.untracked.push(entry.path),
            "D" | "AD" | "MD" => files.deleted.push(entry.path),
            "U" | "UU" | "AA" | "DD" | "AU" | "UA" | "DU" | "UD" => {}
            _ => files.modified.push(entry.path),
        }
    }
    Ok(files)
}

/// Files with unresolved merge conflicts (`UU`, `AA`, ...).
pub fn conflicts(start: &Path) -> Result<Vec<String>> {
    let st = status(start)?;
    Ok(st
        .entries
        .into_iter()
        .filter(|e| {
            matches!(
                e.code.as_str(),
                "UU" | "AA" | "DD" | "AU" | "UA" | "DU" | "UD"
            )
        })
        .map(|e| e.path)
        .collect())
}

/// Run `git` with `args` in `dir`, returning trimmed stdout.
fn git<'a>(dir: &Path, args: impl IntoIterator<Item = &'a str>) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| Error::unsupported(format!("git is not available: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::system(format!(
            "git failed (exit {}): {}",
            output.status.code().unwrap_or(-1),
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_repo() -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("x-core-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        run(&dir, &["init", "-q"]);
        run(&dir, &["config", "user.email", "x@example.com"]);
        run(&dir, &["config", "user.name", "x"]);
        std::fs::write(dir.join("a.txt"), "one").unwrap();
        run(&dir, &["add", "."]);
        run(&dir, &["commit", "-qm", "init"]);
        (dir.clone(), dir)
    }

    fn run(dir: &Path, args: &[&str]) {
        Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git must be installed for these tests");
    }

    #[test]
    fn status_root_branches_and_conflicts_on_a_real_repo() {
        if crate::devenv::which("git").is_none() {
            return; // environment without git
        }
        let (root, workdir) = temp_repo();

        // `git rev-parse --show-toplevel` returns the symlink-resolved path,
        // while `std::env::temp_dir()` may go through a symlink (macOS maps
        // /tmp and /var into /private). Compare canonical forms.
        assert_eq!(
            repo_root(&workdir).unwrap().canonicalize().unwrap(),
            root.canonicalize().unwrap()
        );

        std::fs::write(workdir.join("a.txt"), "two").unwrap();
        std::fs::write(workdir.join("b.txt"), "new").unwrap();
        let st = status(&workdir).unwrap();
        assert!(!st.clean);
        assert!(st
            .entries
            .iter()
            .any(|e| e.path == "a.txt" && e.code.trim() == "M"));
        assert!(st
            .entries
            .iter()
            .any(|e| e.code == "??" && e.path == "b.txt"));

        let changed = changed(&workdir).unwrap();
        assert_eq!(changed.modified, vec!["a.txt".to_string()]);
        assert_eq!(changed.untracked, vec!["b.txt".to_string()]);
        assert!(conflicts(&workdir).unwrap().is_empty());

        let branches = branches(&workdir, false).unwrap();
        assert!(!branches.is_empty());

        let _ = std::fs::remove_dir_all(&root);
    }
}
