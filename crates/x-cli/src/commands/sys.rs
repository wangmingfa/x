//! `x sys`: static facts plus live CPU and memory.

use clap::Subcommand;
use x_core::error::{Error, Result};
use x_core::system::{CpuUsage, MemoryUsage};
use x_core::SystemContext;

use crate::{
    format::{Cell, OutputFormat, Renderer, Table},
    row,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// `x sys` subcommands.
#[derive(Debug, Subcommand)]
pub enum SysCommand {
    /// OS, CPU, memory and uptime.
    Info,

    /// Aggregate and per core CPU utilization.
    Cpu,

    /// Memory utilization.
    Mem,

    /// Poll CPU and memory and print a line per sample.
    Watch {
        /// Seconds between samples.
        #[arg(long, default_value_t = 2.0)]
        interval: f64,

        /// Stop after this many samples; without it, run until Ctrl-C.
        #[arg(long)]
        count: Option<usize>,
    },
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
        SysCommand::Watch { interval, count } => watch(context, renderer, *interval, *count),
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

/// Poll CPU and memory and print one line per sample.
///
/// Unlike `x ps watch`, this prints every sample rather than only what
/// changed: a utilization gauge is a time series, and a flat line would hide
/// exactly the spikes the command exists to show. `--count n` bounds the loop
/// for scripts and tests; without it, run until Ctrl-C.
pub fn watch(
    context: &SystemContext,
    renderer: &mut Renderer,
    interval: f64,
    count: Option<usize>,
) -> Result<i32> {
    let offset = context
        .system
        .info()
        .ok()
        .and_then(|info| info.utc_offset_seconds)
        .unwrap_or(0);
    let interval = if interval.is_finite() {
        interval.max(0.05)
    } else {
        2.0
    };
    let json = renderer.format() == OutputFormat::Json;

    let mut polls = 0usize;
    loop {
        // One family failing must not blind the other: report the failure and
        // keep sampling, because a transient read error is not "the machine is
        // idle".
        let cpu = context.system.cpu_usage();
        let memory = context.system.memory_usage();
        let time = x_core::format_timestamp(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            offset,
        );

        if json {
            renderer.always_json(&serde_json::json!({
                "time": time,
                "cpu": match &cpu {
                    Ok(usage) => serde_json::to_value(usage).unwrap_or(serde_json::Value::Null),
                    Err(err) => serde_json::json!({ "error": err.to_string() }),
                },
                "memory": match &memory {
                    Ok(usage) => serde_json::to_value(usage).unwrap_or(serde_json::Value::Null),
                    Err(err) => serde_json::json!({ "error": err.to_string() }),
                },
            }))?;
        } else {
            renderer.line(format!("[{time}] {}", sample_line(&cpu, &memory)))?;
        }
        renderer.flush().map_err(|e| {
            Error::new(
                x_core::ErrorKind::System,
                format!("watch output failed: {e}"),
            )
        })?;

        polls += 1;
        if count.is_some_and(|target| polls >= target) {
            return Ok(0);
        }
        std::thread::sleep(Duration::from_secs_f64(interval));
    }
}

/// One human-readable sample, naming whichever family failed.
fn sample_line(cpu: &Result<CpuUsage>, memory: &Result<MemoryUsage>) -> String {
    let mut parts: Vec<String> = Vec::new();
    match cpu {
        Ok(usage) => {
            let mut text = format!("cpu {:.1}%", usage.total_percent);
            if let Some(load) = &usage.load_average {
                text.push_str(&format!(" load {load}"));
            }
            if let Some(temp) = usage.temperature_celsius {
                // Labeled, because a bare number right after the load triple
                // reads as a fourth load average. Spelled exactly as `x sys cpu`
                // spells it, so one quantity has one rendering.
                text.push_str(&format!(" temp {temp:.1} C"));
            }
            parts.push(text);
        }
        Err(err) => parts.push(format!("cpu unavailable ({err})")),
    }
    match memory {
        Ok(usage) => parts.push(format!(
            "mem {:.1}% ({} used)",
            usage.percent,
            x_core::format_bytes(usage.used_bytes)
        )),
        Err(err) => parts.push(format!("mem unavailable ({err})")),
    }
    parts.join("  ")
}

/// A fixed width utilization bar, so columns stay aligned in a terminal.
fn bar(percent: f32, width: usize) -> String {
    let ratio = (percent.clamp(0.0, 100.0) / 100.0) as f64;
    let filled = (ratio * width as f64).round() as usize;
    format!("[{}{}]", "#".repeat(filled), "-".repeat(width - filled))
}

#[cfg(test)]
mod tests {
    use super::*;
    use x_core::system::LoadAverage;

    fn cpu(with_load: bool, temperature: Option<f32>) -> CpuUsage {
        CpuUsage {
            total_percent: 12.5,
            load_average: with_load.then_some(LoadAverage {
                one: 1.0,
                five: 2.0,
                fifteen: 3.0,
            }),
            temperature_celsius: temperature,
            ..CpuUsage::default()
        }
    }

    fn memory() -> MemoryUsage {
        MemoryUsage {
            percent: 40.0,
            used_bytes: 4096,
            ..MemoryUsage::default()
        }
    }

    #[test]
    fn the_temperature_is_labeled_so_it_is_not_a_fourth_load_number() {
        let line = sample_line(&Ok(cpu(true, Some(73.4))), &Ok(memory()));
        assert!(line.contains("load 1.00 2.00 3.00"), "{line}");
        assert!(line.contains("temp 73.4 C"), "{line}");
    }

    #[test]
    fn a_sample_without_a_sensor_says_nothing_about_temperature() {
        let line = sample_line(&Ok(cpu(true, None)), &Ok(memory()));
        assert!(!line.contains("temp"), "{line}");
    }

    #[test]
    fn a_failing_family_is_named_without_blinding_the_other() {
        let failure: Result<CpuUsage> = Err(Error::new(x_core::ErrorKind::System, "smc refused"));
        let line = sample_line(&failure, &Ok(memory()));
        assert!(line.contains("cpu unavailable (smc refused)"), "{line}");
        assert!(line.contains("mem 40.0%"), "{line}");
    }
}
