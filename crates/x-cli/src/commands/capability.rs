//! `x capability`: what this host can actually do.
//!
//! The roadmap says what `x` implements; this command asks the running
//! adapters, per feature: supported, degraded, or unsupported.

use clap::Args;
use x_core::error::Result;
use x_core::SystemContext;

use crate::format::{OutputFormat, Renderer, Table};
use crate::row;

/// `x capability` arguments.
#[derive(Debug, Args)]
pub struct CapabilityArgs {
    /// Only report one domain: system, process, port, net, disk or service.
    #[arg(long)]
    pub domain: Option<String>,
}

/// Route a `x capability` invocation.
pub fn dispatch(
    context: &SystemContext,
    renderer: &mut Renderer,
    args: &CapabilityArgs,
) -> Result<i32> {
    let mut rows = x_core::capability::probe(context);
    // net-top's per-process source lives in x-platform, so its capability
    // row is probed there; slot it in with the rest of the net domain.
    let net_top = x_platform::net_top_capability_rows();
    if let Some(pos) = rows.iter().rposition(|row| row.domain == "net") {
        rows.splice(pos + 1..pos + 1, net_top);
    } else {
        rows.extend(net_top);
    }
    if let Some(domain) = &args.domain {
        let needle = domain.to_ascii_lowercase();
        rows.retain(|row| row.domain.to_ascii_lowercase() == needle);
        if rows.is_empty() {
            renderer.line(format!("no capability rows in domain `{domain}`"))?;
            return Ok(0);
        }
    }
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&rows)?;
        return Ok(0);
    }
    let mut table = Table::new(["domain", "feature", "status", "note"]);
    for row in &rows {
        table.push(row![
            row.domain.clone(),
            row.feature.clone(),
            row.status.name().to_string(),
            row.note.clone().unwrap_or_default(),
        ]);
    }
    renderer.table(&table)?;
    Ok(0)
}
