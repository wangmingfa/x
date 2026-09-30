//! `x sys`: static facts plus live CPU and memory.

use clap::Subcommand;
use x_core::error::Result;
use x_core::SystemContext;

use crate::{
    format::{Cell, OutputFormat, Renderer, Table},
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
    if let Some(performance) = info.performance_cores {
        table.push(row!["performance cores", performance.to_string()]);
    }
    if let Some(efficiency) = info.efficiency_cores {
        table.push(row!["efficiency cores", efficiency.to_string()]);
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
    if let (Some(boot_time), Some(offset)) = (info.boot_time, info.utc_offset_seconds) {
        table.push(row![
            "last reboot",
            x_core::format_timestamp(boot_time, offset)
        ]);
    } else if let Some(boot_time) = info.boot_time {
        // Without the offset the timestamp would look local but be UTC.
        table.push(row!["last reboot (utc)", boot_time.to_string()]);
    }
    if let Some(timezone) = &info.timezone {
        table.push(row!["timezone", timezone.clone()]);
    }
    if let Some(locale) = &info.locale {
        table.push(row!["locale", locale.clone()]);
    }
    if let Some(user) = &info.current_user {
        table.push(row!["user", user.clone()]);
    }
    if let Some(shell) = &info.current_shell {
        table.push(row!["shell", shell.clone()]);
    }
    if let Some(terminal) = &info.terminal {
        table.push(row!["terminal", terminal.clone()]);
    }
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
    if let Some(load) = &usage.load_average {
        renderer.line(format!("load {load}"))?;
    }
    if let Some(clock) = usage.frequency_mhz {
        match usage.max_frequency_mhz {
            Some(max) => renderer.line(format!("clock {clock:.0} MHz of {max:.0} MHz"))?,
            None => renderer.line(format!("clock {clock:.0} MHz"))?,
        }
    }
    if let Some(temperature) = usage.temperature_celsius {
        renderer.line(format!("temp {temperature:.1} C"))?;
    }
    if let Some(governor) = &usage.governor {
        renderer.line(format!("governor {governor}"))?;
    }

    // The clock column only appears on platforms that publish one per core.
    let clocks = usage.per_core_frequency_mhz.len() == usage.per_core_percent.len()
        && !usage.per_core_frequency_mhz.is_empty();
    let mut table = if clocks {
        Table::new(["core", "usage", "mhz"])
    } else {
        Table::new(["core", "usage"])
    };
    for (index, value) in usage.per_core_percent.iter().enumerate() {
        let mut row = row![index, bar(*value, 20)];
        if clocks {
            row.push(Cell::from(format!(
                "{:.0}",
                usage.per_core_frequency_mhz[index]
            )));
        }
        table.push(row);
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
    if usage.swap_total_bytes > 0 {
        table.push(row![
            "swap",
            format!(
                "{} used of {}",
                x_core::format_bytes(usage.swap_used_bytes),
                x_core::format_bytes(usage.swap_total_bytes)
            ),
        ]);
    }
    if let Some(pressure) = usage.pressure {
        table.push(row!["pressure", pressure.to_string()]);
    }
    renderer.table(&table)?;
    Ok(0)
}

/// A fixed width utilization bar, so columns stay aligned in a terminal.
fn bar(percent: f32, width: usize) -> String {
    let ratio = (percent.clamp(0.0, 100.0) / 100.0) as f64;
    let filled = (ratio * width as f64).round() as usize;
    format!("[{}{}]", "#".repeat(filled), "-".repeat(width - filled))
}
