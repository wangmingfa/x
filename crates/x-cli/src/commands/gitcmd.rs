//! `x git`: status, branches, changes, conflicts and repo root.

use clap::Subcommand;
use std::path::PathBuf;
use x_core::error::Result;
use x_core::SystemContext;

use crate::{
    format::{OutputFormat, Renderer, Table},
    row,
};

/// `x git` subcommands.
#[derive(Debug, Subcommand)]
pub enum GitCommand {
    /// Working tree summary (branch, ahead/behind, changed files).
    Status {
        /// Repository directory; defaults to the current one.
        #[arg(default_value = ".")]
        path: String,
    },

    /// Local (and optionally remote) branches with their tips.
    Branches {
        /// Include remote-tracking branches.
        #[arg(long, short = 'r')]
        remotes: bool,
        /// Repository directory.
        #[arg(default_value = ".")]
        path: String,
    },

    /// Changed files grouped as modified / untracked / deleted.
    Changed {
        /// Repository directory.
        #[arg(default_value = ".")]
        path: String,
    },

    /// Files with unresolved merge conflicts.
    Conflicts {
        /// Repository directory.
        #[arg(default_value = ".")]
        path: String,
    },

    /// The nearest enclosing repository root.
    Root {
        /// Where to start looking upward.
        #[arg(default_value = ".")]
        path: String,
    },
}

/// Arguments for `x git`.
#[derive(Debug, clap::Args)]
pub struct GitArgs {
    #[command(subcommand)]
    pub command: GitCommand,
}

/// Route a `x git` invocation.
pub fn dispatch(
    _context: &SystemContext,
    renderer: &mut Renderer,
    command: &GitCommand,
) -> Result<i32> {
    match command {
        GitCommand::Status { path } => {
            status(renderer, &PathBuf::from(path))?;
        }
        GitCommand::Branches { remotes, path } => {
            branches(renderer, &PathBuf::from(path), *remotes)?;
        }
        GitCommand::Changed { path } => {
            changed(renderer, &PathBuf::from(path))?;
        }
        GitCommand::Conflicts { path } => {
            conflicts(renderer, &PathBuf::from(path))?;
        }
        GitCommand::Root { path } => {
            let root = x_core::gitcmd::repo_root(&PathBuf::from(path))?;
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&serde_json::json!({ "root": root.display().to_string() }))?;
            } else {
                renderer.line(root.display().to_string())?;
            }
        }
    }
    Ok(0)
}

fn status(renderer: &mut Renderer, path: &std::path::Path) -> Result<i32> {
    let st = x_core::gitcmd::status(path)?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&st)?;
        return Ok(0);
    }
    renderer.line(format!(
        "branch: {}{}",
        st.branch.as_deref().unwrap_or("(detached HEAD)"),
        match (st.ahead, st.behind) {
            (Some(a), Some(b)) => format!(" (ahead {a}, behind {b})"),
            (Some(a), None) => format!(" (ahead {a})"),
            (None, Some(b)) => format!(" (behind {b})"),
            _ => String::new(),
        }
    ))?;
    if st.clean {
        renderer.line("working tree clean")?;
        return Ok(0);
    }
    let mut table = Table::new(["code", "path"]);
    for entry in &st.entries {
        table.push(row![
            entry.code.clone(),
            match &entry.orig_path {
                Some(orig) => format!("{orig} -> {}", entry.path),
                None => entry.path.clone(),
            },
        ]);
    }
    renderer.table(&table)?;
    Ok(0)
}

fn branches(renderer: &mut Renderer, path: &std::path::Path, remotes: bool) -> Result<i32> {
    let list = x_core::gitcmd::branches(path, remotes)?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&list)?;
        return Ok(0);
    }
    let mut table = Table::new(["", "branch", "commit"]);
    for branch in &list {
        table.push(row![
            if branch.current { "*" } else { "" },
            branch.name.clone(),
            branch.commit.clone().unwrap_or_default(),
        ]);
    }
    renderer.table(&table)?;
    Ok(0)
}

fn changed(renderer: &mut Renderer, path: &std::path::Path) -> Result<i32> {
    let files = x_core::gitcmd::changed(path)?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&files)?;
        return Ok(0);
    }
    let mut table = Table::new(["status", "file"]);
    for name in &files.modified {
        table.push(row!["modified", name.clone()]);
    }
    for name in &files.untracked {
        table.push(row!["untracked", name.clone()]);
    }
    for name in &files.deleted {
        table.push(row!["deleted", name.clone()]);
    }
    renderer.table(&table)?;
    Ok(0)
}

fn conflicts(renderer: &mut Renderer, path: &std::path::Path) -> Result<i32> {
    let files = x_core::gitcmd::conflicts(path)?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&files)?;
        return Ok(0);
    }
    if files.is_empty() {
        renderer.line("no conflicts")?;
        return Ok(0);
    }
    let mut table = Table::new(["file"]);
    for name in &files {
        table.push(row![name.clone()]);
    }
    renderer.table(&table)?;
    Ok(0)
}
