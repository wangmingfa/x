//! `x sys`: static facts plus live CPU and memory.

use clap::Subcommand;
use x_core::error::Result;
use x_core::SystemContext;

use crate::{
    format::{OutputFormat, Renderer, Table},
    row,
};

/// `x sys` subcommands.
#[derive(Debug, Subcommand)]
pub enum SysCommand {
    /// OS, CPU, memory and uptime.
    Info,

    /// Aggregate and per core CPU utilization.
    Cpu,

    /// Memory utilization.
    Mem,
}

/// Route a `x sys` invocation.
pub fn dispatch(
    context: &SystemContext,
    renderer: &mut Renderer,
    command: &SysCommand,
) -> Result<i32> {
    match command {
        SysCommand::Info => info(context, renderer),
        SysCommand::Cpu => cpu(context, renderer),
        SysCommand::Mem => mem(context, renderer),
    }
}

/// Static system facts.
pub fn info(context: &SystemContext, renderer: &mut Renderer) -> Result<i32> {
    let info = context.system.info()?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&info)?;
        return Ok(0);
    }

    let mut table = Table::new(["field", "value"]);
    table.push(row!["os", format!("{} {}", info.os_name, info.os_version)]);
    if let Some(kernel) = &info.kernel_version {
        table.push(row!["kernel", kernel.clone()]);
    }
    table.push(row!["arch", info.arch.clone()]);
    table.push(row!["hostname", info.hostname.clone()]);
    table.push(row![
        "cpu",
        info.cpu_brand.clone().unwrap_or_else(|| "-".into()),
    ]);
    table.push(row!["logical cpus", info.cpu_count.to_string()]);
    if let Some(physical) = info.physical_cores {
        table.push(row!["physical cores", physical.to_string()]);
    }
    table.push(row![
        "memory",
        format!(
            "{} used of {}",
            x_core::format_bytes(info.total_memory_bytes - info.available_memory_bytes),
            x_core::format_bytes(info.total_memory_bytes)
        ),
    ]);
    table.push(row!["uptime", x_core::format_duration(info.uptime_seconds)]);
    renderer.table(&table)?;
    Ok(0)
}

/// Live CPU utilization.
pub fn cpu(context: &SystemContext, renderer: &mut Renderer) -> Result<i32> {
    let usage = context.system.cpu_usage()?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&usage)?;
        return Ok(0);
    }

    renderer.line(format!(
        "total {:.1}% {}",
        usage.total_percent,
        bar(usage.total_percent, 30)
    ))?;
    let mut table = Table::new(["core", "usage"]);
    for (index, value) in usage.per_core_percent.iter().enumerate() {
        table.push(row![index, bar(*value, 20)]);
    }
    renderer.table(&table)?;
    Ok(0)
}

/// Live memory utilization.
pub fn mem(context: &SystemContext, renderer: &mut Renderer) -> Result<i32> {
    let usage = context.system.memory_usage()?;
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&usage)?;
        return Ok(0);
    }
    renderer.line(format!("{:.1}% {}", usage.percent, bar(usage.percent, 30)))?;
    let mut table = Table::new(["field", "value"]);
    table.push(row!["total", x_core::format_bytes(usage.total_bytes)]);
    table.push(row!["used", x_core::format_bytes(usage.used_bytes)]);
    table.push(row![
        "available",
        x_core::format_bytes(usage.available_bytes)
    ]);
    renderer.table(&table)?;
    Ok(0)
}

/// A fixed width utilization bar, so columns stay aligned in a terminal.
fn bar(percent: f32, width: usize) -> String {
    let ratio = (percent.clamp(0.0, 100.0) / 100.0) as f64;
    let filled = (ratio * width as f64).round() as usize;
    format!("[{}{}]", "#".repeat(filled), "-".repeat(width - filled))
}
