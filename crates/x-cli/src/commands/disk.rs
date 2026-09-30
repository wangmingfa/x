//! `x disk`: mounted filesystems.

use clap::Subcommand;
use x_core::error::Result;
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
    let mut table = Table::new(["mount", "device", "fs", "size", "used", "available", "use%"]);
    for row in &rows {
        table.push(row![
            row.mount_point.clone(),
            row.name.clone().unwrap_or_else(|| "-".into()),
            row.file_system.clone().unwrap_or_else(|| "-".into()),
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
