//! `x file`: inspect and operate on files.
//!
//! Inspection verbs (`info`, `size`, `list`) are read-only. The operating
//! verbs (`copy`, `move`, `rename`, `trash`, `open`, `reveal`) change the
//! machine — `trash` / `open` / `reveal` go through the platform adapter so
//! each OS does the natural thing.

use clap::Subcommand;
use x_core::error::{Error, Result};
use x_core::SystemContext;

use crate::{
    format::{Confirmer, OutputFormat, Renderer, Table},
    row,
};

/// `x file` subcommands.
#[derive(Debug, Subcommand)]
pub enum FileCommand {
    /// Metadata for one path.
    Info {
        /// Path to inspect.
        path: String,
    },

    /// Byte size; directories are counted recursively.
    Size {
        /// Path to measure.
        path: String,
    },

    /// Hash one file.
    Hash {
        /// File to read. A directory is refused rather than hashed as a tree.
        path: String,

        /// Checksum algorithm; sha256 when omitted.
        #[arg(long, short = 'a', value_enum, default_value_t = AlgoArg::Sha256)]
        algo: AlgoArg,
    },

    /// Names inside a directory.
    List {
        /// Directory to list.
        path: String,
    },

    /// Open the path with its default application.
    Open {
        /// Path to open.
        path: String,
        /// Do not ask for confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Show the path in the platform file manager (Finder / Explorer).
    Reveal {
        /// Path to reveal.
        path: String,
        /// Do not ask for confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Move the path to the platform trash / recycle bin.
    Trash {
        /// Path to trash.
        path: String,
        /// Do not ask for confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Copy a file or directory tree.
    Copy {
        /// Source.
        from: String,
        /// Destination (created; existing files are overwritten).
        to: String,
        /// Do not ask for confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Move (or rename) a file or directory.
    Move {
        /// Source.
        from: String,
        /// Destination.
        to: String,
        /// Do not ask for confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },

    /// Rename within the same directory.
    Rename {
        /// Path to rename.
        path: String,
        /// New file name (no separators).
        new_name: String,
        /// Do not ask for confirmation.
        #[arg(long, short = 'y')]
        yes: bool,
    },
}

/// Checksum algorithm as spelled on the command line.
#[derive(Debug, Clone, Copy, Default, clap::ValueEnum)]
pub enum AlgoArg {
    /// SHA-256, the interoperability default.
    #[default]
    Sha256,
    /// SHA-512, the wider digest.
    Sha512,
    /// CRC-32, the fast accidental-corruption check.
    Crc32,
}

impl From<AlgoArg> for x_core::file::HashAlgo {
    fn from(value: AlgoArg) -> Self {
        match value {
            AlgoArg::Sha256 => x_core::file::HashAlgo::Sha256,
            AlgoArg::Sha512 => x_core::file::HashAlgo::Sha512,
            AlgoArg::Crc32 => x_core::file::HashAlgo::Crc32,
        }
    }
}

/// Arguments for `x file <sub>`.
#[derive(Debug, clap::Args)]
pub struct FileArgs {
    #[command(subcommand)]
    pub command: FileCommand,
}

/// Route a `x file` invocation.
pub fn dispatch(
    context: &SystemContext,
    renderer: &mut Renderer,
    confirmer: &mut dyn Confirmer,
    command: &FileCommand,
) -> Result<i32> {
    let file = context
        .file
        .as_ref()
        .ok_or_else(|| Error::unsupported("file operations are not available in this context"))?;

    match command {
        FileCommand::Info { path } => {
            info(file.as_ref(), renderer, path)?;
        }
        FileCommand::Size { path } => {
            size(file.as_ref(), renderer, path)?;
        }
        FileCommand::Hash { path, algo } => {
            hash(renderer, path, (*algo).into())?;
        }
        FileCommand::List { path } => {
            list(file.as_ref(), renderer, path)?;
        }
        FileCommand::Open { path, yes } => {
            if crate::dry_run_guard(renderer, &format!("open {path}"))? {
                return Ok(0);
            }
            if !super::confirm(renderer, confirmer, *yes, &format!("open {path}"))? {
                return Ok(super::EXIT_DECLINED);
            }
            file.open(path.as_ref())?;
            renderer.line(format!("opened {path}"))?;
        }
        FileCommand::Reveal { path, yes } => {
            if crate::dry_run_guard(renderer, &format!("reveal {path}"))? {
                return Ok(0);
            }
            if !super::confirm(renderer, confirmer, *yes, &format!("reveal {path}"))? {
                return Ok(super::EXIT_DECLINED);
            }
            file.reveal(path.as_ref())?;
            renderer.line(format!("revealed {path}"))?;
        }
        FileCommand::Trash { path, yes } => {
            if crate::dry_run_guard(renderer, &format!("move {path} to trash"))? {
                return Ok(0);
            }
            if !super::confirm(renderer, confirmer, *yes, &format!("trash {path}"))? {
                return Ok(super::EXIT_DECLINED);
            }
            file.trash(path.as_ref())?;
            renderer.line(format!("moved {path} to trash"))?;
        }
        FileCommand::Copy { from, to, yes } => {
            if crate::dry_run_guard(renderer, &format!("copy {from} to {to}"))? {
                return Ok(0);
            }
            if !super::confirm(renderer, confirmer, *yes, &format!("copy {from} to {to}"))? {
                return Ok(super::EXIT_DECLINED);
            }
            file.copy(from.as_ref(), to.as_ref())?;
            renderer.line(format!("copied {from} to {to}"))?;
        }
        FileCommand::Move { from, to, yes } => {
            if crate::dry_run_guard(renderer, &format!("move {from} to {to}"))? {
                return Ok(0);
            }
            if !super::confirm(renderer, confirmer, *yes, &format!("move {from} to {to}"))? {
                return Ok(super::EXIT_DECLINED);
            }
            file.move_path(from.as_ref(), to.as_ref())?;
            renderer.line(format!("moved {from} to {to}"))?;
        }
        FileCommand::Rename {
            path,
            new_name,
            yes,
        } => {
            if crate::dry_run_guard(renderer, &format!("rename {path} to {new_name}"))? {
                return Ok(0);
            }
            if !super::confirm(
                renderer,
                confirmer,
                *yes,
                &format!("rename {path} to {new_name}"),
            )? {
                return Ok(super::EXIT_DECLINED);
            }
            file.rename(path.as_ref(), new_name)?;
            renderer.line(format!("renamed {path} to {new_name}"))?;
        }
    }
    Ok(0)
}

fn info(file: &dyn x_core::file::FileManager, renderer: &mut Renderer, path: &str) -> Result<i32> {
    let info = file.info(path.as_ref())?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&info)?;
        return Ok(0);
    }
    let mut table = Table::new(["field", "value"]);
    table.push(row!["path", info.path.display().to_string()]);
    table.push(row!["type", info.type_label()]);
    if let Some(size) = info.size {
        table.push(row!["size", x_core::format_bytes(size)]);
    }
    if let Some(mode) = &info.mode {
        table.push(row!["mode", mode.clone()]);
    }
    table.push(row!["readonly", info.readonly.to_string()]);
    if let Some(owner) = &info.owner {
        table.push(row!["owner", owner.clone()]);
    }
    if let Some(group) = &info.group {
        table.push(row!["group", group.clone()]);
    }
    if let Some(modified) = &info.modified {
        table.push(row!["modified", modified.clone()]);
    }
    if let Some(created) = &info.created {
        table.push(row!["created", created.clone()]);
    }
    if let Some(target) = &info.target {
        table.push(row!["target", target.display().to_string()]);
    }
    renderer.table(&table)?;
    Ok(0)
}

fn size(file: &dyn x_core::file::FileManager, renderer: &mut Renderer, path: &str) -> Result<i32> {
    let bytes = file.size(path.as_ref())?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&serde_json::json!({ "path": path, "bytes": bytes }))?;
        return Ok(0);
    }
    renderer.line(format!(
        "{path}: {} ({bytes} bytes)",
        x_core::format_bytes(bytes)
    ))?;
    Ok(0)
}

/// Hash one file.
///
/// Pure read, so no confirmation and no audit line. The digest is printed in
/// the `sha256sum` shape (`<hex>  <path>`) because that is what every other
/// tool emits, so a user can pipe or diff it against the platform's own.
fn hash(renderer: &mut Renderer, path: &str, algo: x_core::file::HashAlgo) -> Result<i32> {
    let digest = x_core::file::hash_file(path.as_ref(), algo)?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&digest)?;
        return Ok(0);
    }
    renderer.line(format!("{}  {}", digest.hex, path))?;
    Ok(0)
}

fn list(file: &dyn x_core::file::FileManager, renderer: &mut Renderer, path: &str) -> Result<i32> {
    let entries = file.list(path.as_ref())?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&entries)?;
        return Ok(0);
    }
    let mut table = Table::new(["name", "type", "size"]);
    for entry in &entries {
        table.push(row![
            entry.name.clone(),
            entry.file_type.name().to_string(),
            entry
                .size
                .map(x_core::format_bytes)
                .unwrap_or_else(|| "-".into()),
        ]);
    }
    renderer.table(&table)?;
    Ok(0)
}
