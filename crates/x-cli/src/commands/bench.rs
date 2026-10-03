//! `x bench`: how long the listing reads take on this machine.
//!
//! `x capability` says whether a read works here; this says what it costs, so
//! an adapter that quietly became expensive shows up as a `slow` row instead of
//! as a user complaint. Timings come from the real platform adapters and are
//! divided by the rows they actually returned, which keeps a laptop and a server
//! comparable.

use clap::Args;
use x_core::bench::{self, BenchConfig};
use x_core::error::{Error, Result};
use x_core::SystemContext;

use crate::format::{OutputFormat, Renderer, Table};
use crate::row;

/// `x bench` arguments.
#[derive(Debug, Args)]
pub struct BenchArgs {
    /// Timed passes per target; one untimed pass warms the adapter first.
    #[arg(long, default_value_t = 3)]
    pub samples: usize,

    /// Only measure one domain: process, port, net, disk or service.
    #[arg(long)]
    pub domain: Option<String>,

    /// Judge every target against this budget instead of its own.
    #[arg(long)]
    pub budget_ms: Option<f64>,

    /// Exit 1 when a target goes over budget, for CI regression gates.
    #[arg(long)]
    pub check: bool,
}

/// Route a `x bench` invocation.
pub fn dispatch(context: &SystemContext, renderer: &mut Renderer, args: &BenchArgs) -> Result<i32> {
    if args.samples == 0 {
        return Err(Error::invalid_input("--samples must be at least 1"));
    }
    if let Some(budget) = args.budget_ms {
        if !(budget.is_finite() && budget >= 0.0) {
            return Err(Error::invalid_input(
                "--budget-ms must be a finite, non-negative number",
            ));
        }
    }

    let report = bench::bench(
        context,
        &BenchConfig {
            samples: args.samples,
            budget_ms: args.budget_ms,
            domain: args.domain.clone(),
        },
    );

    if renderer.format() == OutputFormat::Json {
        // One document: the samples are numbers, so the table path (which
        // stringifies every cell) would lose their type.
        renderer.always_json(&report)?;
    } else {
        let mut table = Table::new([
            "target",
            "rows",
            "best ms",
            "median ms",
            "us/row",
            "budget ms",
            "status",
            "note",
        ]);
        for row in &report.rows {
            table.push(row![
                row.target.label(),
                row.rows.to_string(),
                milliseconds(row.best_ms),
                milliseconds(row.median_ms),
                microseconds(row.us_per_row),
                milliseconds(Some(row.budget_ms)),
                row.status.name(),
                row.note.clone().unwrap_or_default(),
            ]);
        }
        renderer.table(&table)?;
        for note in &report.scale_notes {
            renderer.line(format!("note: {note}"))?;
        }
    }

    if args.check {
        let slow = report.slow_rows();
        if !slow.is_empty() {
            if renderer.format() != OutputFormat::Json {
                let names: Vec<&str> = slow.iter().map(|row| row.target.label()).collect();
                renderer.line(format!(
                    "{} target(s) over budget: {}",
                    slow.len(),
                    names.join(", ")
                ))?;
            }
            return Ok(1);
        }
    }
    Ok(0)
}

fn milliseconds(value: Option<f64>) -> String {
    value
        .map(|ms| format!("{ms:.2}"))
        .unwrap_or_else(|| "-".to_string())
}

/// Per-row cost is printed with one decimal: below a microsecond the difference
/// between two adapters is noise, and a long decimal makes it look meaningful.
fn microseconds(value: Option<f64>) -> String {
    value
        .map(|us| format!("{us:.1}"))
        .unwrap_or_else(|| "-".to_string())
}
