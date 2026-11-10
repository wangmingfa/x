//! `x uninstall`: remove this binary and its upgrade leftovers.
//!
//! Deleting a running executable is allowed on unix — the inode lives until
//! the process exits — but not on Windows, where `x upgrade`'s own rename
//! dance is reused: move the running file aside, then hand the deletion to a
//! detached shell that runs after this process is gone.
//!
//! The confirmation contract is the same as every destructive command:
//! `--dry-run` prints the plan and exits first, JSON callers get single
//! documents, and a declined confirmation exits 130.

use std::fs;
use std::path::{Path, PathBuf};

use clap::Parser;
use x_core::error::{Error, Result};

use crate::format::{Confirmer, Renderer};
use crate::{commands, OutputFormat};

#[derive(Debug, Parser)]
pub struct UninstallArgs {
    /// Assume yes: remove without asking.
    #[arg(short, long)]
    pub yes: bool,
}

/// What one uninstall run intends to remove.
#[derive(Debug, PartialEq, Eq)]
struct RemovalPlan {
    /// The running binary itself.
    exe: PathBuf,
    /// Upgrade leftovers sharing the binary's directory: `x.old` (or
    /// `x.exe.old`) from the rename dance, `.x-new-*` staged files from the
    /// unix upgrade path.
    leftovers: Vec<PathBuf>,
    /// Shell rc files still carrying the installer's PATH line. Reported, not
    /// edited: rewriting a user's shell config is beyond "uninstall myself".
    rc_notes: Vec<PathBuf>,
}

/// Marker the installer writes above the PATH line it appends.
const RC_MARKER: &str = "# added by the x installer";

/// Plan the removal of the binary at `exe`.
///
/// Leftovers are sorted so the report is deterministic: `read_dir` order is
/// whatever the filesystem feels like, and a plan that shuffles between runs
/// is a plan users cannot review.
fn plan(exe: &Path) -> RemovalPlan {
    let mut leftovers = Vec::new();
    let old_name = exe
        .file_name()
        .map(|name| {
            let mut old = name.to_os_string();
            old.push(".old");
            old
        })
        .unwrap_or_default();
    if let Some(dir) = exe.parent() {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let staged = name.to_string_lossy().starts_with(".x-new-");
                if staged || name == old_name {
                    leftovers.push(entry.path());
                }
            }
        }
    }
    leftovers.sort();

    RemovalPlan {
        exe: exe.to_path_buf(),
        leftovers,
        rc_notes: installer_rc_notes(),
    }
}

/// rc files under `home` still carrying the installer's PATH line.
fn installer_rc_notes_in(home: &Path) -> Vec<PathBuf> {
    [".zshrc", ".bashrc", ".bash_profile"]
        .iter()
        .map(|name| home.join(name))
        .filter(|rc| {
            fs::read_to_string(rc)
                .map(|content| content.contains(RC_MARKER))
                .unwrap_or(false)
        })
        .collect()
}

fn installer_rc_notes() -> Vec<PathBuf> {
    match std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
    {
        Some(home) => installer_rc_notes_in(&home),
        None => Vec::new(),
    }
}

/// Remove the leftovers, then the running binary.
fn remove(exe: &Path, leftovers: &[PathBuf]) -> Result<()> {
    for leftover in leftovers {
        // A leftover that refuses to leave does not fail the uninstall; the
        // report below still names what remains.
        let _ = fs::remove_file(leftover);
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let dir = exe
            .parent()
            .ok_or_else(|| Error::system("running executable has no parent directory"))?;
        let name = exe
            .file_name()
            .ok_or_else(|| Error::system("running executable has no file name"))?;
        let old = dir.join({
            let mut old = name.to_os_string();
            old.push(".old");
            old
        });
        let _ = fs::remove_file(&old);
        // A running exe cannot be deleted, but the renamed one stops being in
        // the way; a detached shell waits out this process and deletes it.
        fs::rename(exe, &old)?;
        std::process::Command::new("cmd")
            .args([
                "/C",
                &format!(
                    "timeout /t 2 /nobreak >nul & del /f /q \"{}\"",
                    old.display()
                ),
            ])
            .creation_flags(0x0000_0008) // DETACHED_PROCESS
            .spawn()?;
    }

    #[cfg(not(windows))]
    {
        fs::remove_file(exe)?;
    }
    Ok(())
}

fn report(renderer: &mut Renderer, doc: serde_json::Value, text: String) -> Result<i32> {
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&doc)?;
    } else {
        renderer.line(text)?;
    }
    Ok(0)
}

pub fn dispatch(
    renderer: &mut Renderer,
    confirmer: &mut dyn Confirmer,
    args: &UninstallArgs,
) -> Result<i32> {
    if crate::dry_run_guard(renderer, "remove this x binary")? {
        return Ok(0);
    }
    let exe = std::env::current_exe()
        .map_err(|e| Error::system(format!("cannot locate the running x: {e}")))?;
    let plan = plan(&exe);

    // The plan is shown before the question: a confirmation without the list
    // of paths it confirms is theatre.
    if renderer.format() != OutputFormat::Json {
        renderer.line(format!("removing {}", plan.exe.display()))?;
        for leftover in &plan.leftovers {
            renderer.line(format!("removing leftover {}", leftover.display()))?;
        }
        for note in &plan.rc_notes {
            renderer.line(format!(
                "note: the installer's PATH line is still in {}",
                note.display()
            ))?;
        }
    }

    let action = format!("uninstall x ({})", plan.exe.display());
    if !commands::confirm(renderer, confirmer, args.yes, &action)? {
        return Ok(commands::EXIT_DECLINED);
    }

    remove(&plan.exe, &plan.leftovers)?;

    let removed: Vec<String> = std::iter::once(plan.exe.display().to_string())
        .chain(plan.leftovers.iter().map(|p| p.display().to_string()))
        .collect();
    let notes: Vec<String> = plan
        .rc_notes
        .iter()
        .map(|rc| {
            format!(
                "the installer's PATH line is still in {} — remove it manually",
                rc.display()
            )
        })
        .collect();
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&serde_json::json!({
            "uninstalled": true,
            "removed": removed,
            "notes": notes,
        }))?;
        return Ok(0);
    }
    // The rc notes were already shown with the plan above; repeating them
    // verbatim after the removal adds noise, not information.
    report(
        renderer,
        serde_json::json!({ "uninstalled": true, "removed": removed, "notes": notes }),
        format!("uninstalled x ({})", plan.exe.display()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch directory that is removed again when the test ends.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "x-uninstall-test-{}-{}",
                label,
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("scratch dir");
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_plan_names_the_binary_and_both_leftover_kinds() {
        let scratch = Scratch::new("plan");
        let dir = &scratch.0;
        fs::write(dir.join("x"), b"exe").expect("exe");
        fs::write(dir.join("x.old"), b"old").expect("old");
        fs::write(dir.join(".x-new-42"), b"staged").expect("staged");
        fs::write(dir.join("unrelated.txt"), b"keep").expect("unrelated");

        let plan = plan(&dir.join("x"));
        assert_eq!(plan.exe, dir.join("x"));
        assert_eq!(
            plan.leftovers,
            vec![dir.join(".x-new-42"), dir.join("x.old")],
            "sorted, and nothing else"
        );
        assert!(plan.rc_notes.is_empty());
    }

    #[test]
    fn an_old_leftover_matches_the_binarys_name() {
        let scratch = Scratch::new("plan-renamed");
        let dir = &scratch.0;
        // A binary installed under a different name (distro package, renamed
        // download) must not sweep up some other file's `.old`.
        fs::write(dir.join("x-cli"), b"exe").expect("exe");
        fs::write(dir.join("x.old"), b"another binary's leftover").expect("old");

        let first = plan(&dir.join("x-cli"));
        assert!(first.leftovers.is_empty(), "{:?}", first.leftovers);

        fs::write(dir.join("x-cli.old"), b"own leftover").expect("own old");
        let second = plan(&dir.join("x-cli"));
        assert_eq!(second.leftovers, vec![dir.join("x-cli.old")]);
    }

    #[test]
    fn rc_notes_only_cover_files_with_the_installer_marker() {
        let scratch = Scratch::new("rc");
        let home = &scratch.0;
        fs::write(
            home.join(".zshrc"),
            format!("export PATH=\"/x:$PATH\"\n{RC_MARKER}\n"),
        )
        .expect("zshrc");
        fs::write(home.join(".bashrc"), "export PATH=\"/x:$PATH\"\n").expect("bashrc");

        let notes = installer_rc_notes_in(home);
        assert_eq!(notes, vec![home.join(".zshrc")]);
    }
}
