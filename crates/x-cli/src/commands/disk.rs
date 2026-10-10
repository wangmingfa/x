//! `x disk`: mounted filesystems and directory usage.

use std::path::PathBuf;

use clap::Subcommand;
use x_core::error::{Error, Result};
use x_core::SystemContext;

use crate::{
    format::{OutputFormat, Renderer, Table},
    row,
};

/// `x disk` subcommands.
#[derive(Debug, Subcommand)]
pub enum DiskCommand {
    /// All mounted filesystems.
    List,

    /// The filesystem holding the current directory.
    Current,

    /// Block-device I/O rates per whole disk, live.
    Io(IoArgs),

    /// Recursive disk usage of a directory, like `du`.
    Usage {
        /// The directory to size up.
        path: PathBuf,

        /// Only report directories up to this many levels below the root.
        #[arg(long, value_name = "N")]
        depth: Option<usize>,
    },
}

/// `x disk io` arguments.
#[derive(Debug, clap::Args)]
pub struct IoArgs {
    /// Seconds between rounds; also the counting window.
    #[arg(long, default_value_t = 2.0)]
    pub interval: f64,

    /// Stop after this many rounds instead of running until interrupted.
    #[arg(long)]
    pub count: Option<usize>,

    /// Sort key: read, write, total or device.
    #[arg(long, default_value = "total")]
    pub sort: String,
}

/// Route a `x disk` invocation.
pub fn dispatch(
    context: &SystemContext,
    renderer: &mut Renderer,
    command: &DiskCommand,
) -> Result<i32> {
    match command {
        DiskCommand::List => list(context, renderer),
        DiskCommand::Current => current(context, renderer),
        DiskCommand::Io(args) => disk_io(context, renderer, args),
        DiskCommand::Usage { path, depth } => usage(renderer, path, *depth),
    }
}

/// Live block-device I/O rates: sample cumulative counters, diff consecutive
/// rounds — the same snapshot-diff loop `x net top` runs. The window between
/// rounds *is* the counting window, so no extra sleep.
pub fn disk_io(context: &SystemContext, renderer: &mut Renderer, args: &IoArgs) -> Result<i32> {
    let interval = if args.interval.is_finite() && args.interval > 0.0 {
        args.interval.max(0.05)
    } else {
        2.0
    };
    let json = matches!(renderer.format(), OutputFormat::Json | OutputFormat::Jsonl);

    // A platform without a source fails on the very first read, before the
    // loop can claim it is working.
    let first = context.disk.io()?;
    if first.is_empty() {
        return Err(Error::unsupported("no block devices reported I/O counters"));
    }

    let mut previous = first;
    if !json {
        renderer.line(format!("disk io: sampling every {interval}s"))?;
    }
    let mut rounds = 0usize;
    loop {
        // Sleep *before* sampling: the window between two reads is the
        // counting window, and the first round needs one too.
        std::thread::sleep(std::time::Duration::from_secs_f64(interval));
        let current = context.disk.io()?;
        let mut rates: Vec<_> = current
            .iter()
            .map(|row| {
                let prior = previous
                    .iter()
                    .find(|p| p.device == row.device)
                    .unwrap_or(row);
                x_core::diff_disk_io(prior, row, interval)
            })
            .collect();
        sort_disk_io(&mut rates, &args.sort);

        if json {
            render_disk_io_json(renderer, &rates, interval, rounds)?;
        } else {
            render_disk_io_table(renderer, &rates)?;
        }
        renderer.flush().map_err(|e| {
            Error::new(
                x_core::ErrorKind::System,
                format!("disk io output failed: {e}"),
            )
        })?;

        previous = current;
        rounds += 1;
        if args.count.is_some_and(|target| rounds >= target) {
            return Ok(0);
        }
    }
}

/// `--sort` orders by; ties break by device name.
fn sort_disk_io(rates: &mut [x_core::DiskIoRates], key: &str) {
    rates.sort_by(|a, b| match key {
        "read" => a
            .read_bytes_per_sec
            .total_cmp(&b.read_bytes_per_sec)
            .reverse(),
        "write" => a
            .write_bytes_per_sec
            .total_cmp(&b.write_bytes_per_sec)
            .reverse(),
        "device" => a.device.cmp(&b.device),
        _ => {
            let ta = a.read_bytes_per_sec + a.write_bytes_per_sec;
            let tb = b.read_bytes_per_sec + b.write_bytes_per_sec;
            if ta != tb {
                return ta.total_cmp(&tb).reverse();
            }
            a.device.cmp(&b.device)
        }
    });
}

/// JSON: one document per round (Json) or one per line (Jsonl).
fn render_disk_io_json(
    renderer: &mut Renderer,
    rates: &[x_core::DiskIoRates],
    interval: f64,
    round: usize,
) -> Result<()> {
    let mut doc = serde_json::Map::new();
    doc.insert("round".into(), serde_json::json!(round));
    doc.insert("interval_s".into(), serde_json::json!(interval));
    doc.insert("disks".into(), serde_json::json!(rates));
    renderer.json(&doc).map_err(|e| {
        Error::new(
            x_core::ErrorKind::System,
            format!("disk io json failed: {e}"),
        )
    })?;
    Ok(())
}

/// Table: rates per whole disk, busiest first.
fn render_disk_io_table(renderer: &mut Renderer, rates: &[x_core::DiskIoRates]) -> Result<()> {
    let mut table = Table::new(["device", "read/s", "write/s", "read ops/s", "write ops/s"]);
    for rate in rates {
        table.push(row![
            rate.device.clone(),
            rate_cell(rate.read_bytes_per_sec),
            rate_cell(rate.write_bytes_per_sec),
            rate_ops(rate.read_ops_per_sec),
            rate_ops(rate.write_ops_per_sec),
        ]);
    }
    renderer.table(&table)?;
    Ok(())
}

/// Bytes-per-second as a human rate; a zero interval reads `-`, not `0`.
fn rate_cell(bps: f64) -> String {
    if bps <= 0.0 {
        "-".into()
    } else {
        format!("{}/s", x_core::format_bytes(bps as u64))
    }
}

/// Operations-per-second, whole numbers are enough at this scale.
fn rate_ops(ops_per_sec: f64) -> String {
    if ops_per_sec <= 0.0 {
        "-".into()
    } else {
        format!("{:.0}/s", ops_per_sec)
    }
}

/// All mounted filesystems, busiest first.
pub fn list(context: &SystemContext, renderer: &mut Renderer) -> Result<i32> {
    let mut rows = context.disk.list()?;
    rows.sort_by(|a, b| b.percent.total_cmp(&a.percent));
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&rows)?;
        return Ok(0);
    }
    let mut table = Table::new([
        "mount",
        "device",
        "fs",
        "media",
        "size",
        "used",
        "available",
        "use%",
    ]);
    for row in &rows {
        table.push(row![
            row.mount_point.clone(),
            row.name.clone().unwrap_or_else(|| "-".into()),
            row.file_system.clone().unwrap_or_else(|| "-".into()),
            row.media_type
                .map(|media| media.to_string())
                .unwrap_or_else(|| "-".into()),
            x_core::format_bytes(row.total_bytes),
            x_core::format_bytes(row.used_bytes()),
            x_core::format_bytes(row.available_bytes),
            format!("{:.0}%", row.percent),
        ]);
    }
    renderer.table(&table)?;
    Ok(0)
}

/// The filesystem holding the current working directory.
pub fn current(context: &SystemContext, renderer: &mut Renderer) -> Result<i32> {
    let row = context.disk.current()?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&row)?;
        return Ok(0);
    }
    let mut table = Table::new(["field", "value"]);
    table.push(row!["mount", row.mount_point.clone()]);
    table.push(row![
        "device",
        row.name.clone().unwrap_or_else(|| "-".into()),
    ]);
    table.push(row![
        "fs",
        row.file_system.clone().unwrap_or_else(|| "-".into()),
    ]);
    table.push(row!["size", x_core::format_bytes(row.total_bytes)]);
    table.push(row!["used", x_core::format_bytes(row.used_bytes())]);
    table.push(row!["available", x_core::format_bytes(row.available_bytes)]);
    table.push(row!["use%", format!("{:.0}%", row.percent)]);
    if row.read_only == Some(true) {
        table.push(row!["read only", "yes"]);
    }
    renderer.table(&table)?;
    Ok(0)
}

/// Directory usage: every subdirectory with its aggregated size, biggest
/// first, like `du -a --max-depth`.
pub fn usage(renderer: &mut Renderer, path: &std::path::Path, depth: Option<usize>) -> Result<i32> {
    // `walk_directory` reports an unreadable root as an empty tree; `du`
    // exits non-zero on a missing path, and that is the contract here too.
    let metadata = std::fs::symlink_metadata(path).map_err(|err| match err.kind() {
        std::io::ErrorKind::NotFound => {
            Error::not_found(format!("no such file or directory: {}", path.display()))
        }
        _ => Error::from(err),
    })?;
    let rows = if metadata.is_dir() {
        x_core::walk_directory(path, depth)
    } else {
        // `du` on a plain file sizes just that file.
        vec![x_core::DirUsage {
            path: path.to_path_buf(),
            depth: 0,
            total_bytes: metadata.len(),
            files: 1,
            dirs: 0,
            unreadable: 0,
        }]
    };
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&rows)?;
        return Ok(0);
    }
    let mut table = Table::new(["path", "size", "files", "dirs", "unreadable"]);
    for row in &rows {
        table.push(row![
            row.path.display().to_string(),
            x_core::format_bytes(row.total_bytes),
            row.files.to_string(),
            row.dirs.to_string(),
            row.unreadable.to_string(),
        ]);
    }
    renderer.table(&table)?;
    Ok(0)
}
