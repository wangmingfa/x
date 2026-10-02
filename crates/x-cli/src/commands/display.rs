//! `x display`: the machine's display topology.
//!
//! Pure reads — no confirmation, no elevation, no audit. Fields the
//! platform does not report (position on Wayland, scaling on X11) are
//! shown as `-` and omitted from JSON rather than guessed.

use clap::Subcommand;
use x_core::display::DisplayInfo;
use x_core::error::{Error, Result};
use x_core::SystemContext;

use crate::format::{OutputFormat, Renderer, Table};
use crate::row;

/// `x display` subcommands.
#[derive(Debug, Subcommand)]
pub enum DisplayCommand {
    /// Every display the platform reports.
    List,

    /// One display in full: index from `list`, or a name.
    Info(InfoArgs),
}

/// Arguments for `x display info`.
#[derive(Debug, clap::Args)]
pub struct InfoArgs {
    /// Row number from `x display list` (1-based), or a name.
    pub target: String,
}

/// Arguments for `x display`.
#[derive(Debug, clap::Args)]
pub struct DisplayArgs {
    #[command(subcommand)]
    pub command: DisplayCommand,
}

/// Route a `x display` invocation.
pub fn dispatch(
    context: &SystemContext,
    renderer: &mut Renderer,
    command: &DisplayCommand,
) -> Result<i32> {
    let display = context
        .display
        .as_ref()
        .ok_or_else(|| Error::unsupported("no display capability in this context"))?;

    match command {
        DisplayCommand::List => {
            let rows = display.displays()?;
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&rows)?;
                return Ok(0);
            }
            if rows.is_empty() {
                renderer.line("no displays reported by the platform")?;
                return Ok(0);
            }
            let mut table = Table::new([
                "name",
                "connected",
                "resolution",
                "refresh",
                "scale",
                "hdr",
                "primary",
                "position",
            ]);
            for info in &rows {
                table.push(row![
                    info.name.clone(),
                    yes_no(info.connected),
                    resolution(info),
                    refresh(info),
                    scale(info),
                    hdr(info),
                    yes_no(info.primary),
                    position(info),
                ]);
            }
            renderer.table(&table)?;
        }
        DisplayCommand::Info(args) => {
            let rows = display.displays()?;
            let info = select(&rows, &args.target)?;
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(info)?;
                return Ok(0);
            }
            let mut table = Table::new(["field", "value"]);
            table.push(row!["name", info.name.clone()]);
            table.push(row!["id", info.id.clone().unwrap_or_else(|| "-".into())]);
            table.push(row!["connected", yes_no(info.connected)]);
            table.push(row!["resolution", resolution(info)]);
            table.push(row!["refresh", refresh(info)]);
            table.push(row!["scale", scale(info)]);
            table.push(row!["hdr", hdr(info)]);
            table.push(row!["primary", yes_no(info.primary)]);
            table.push(row!["position", position(info)]);
            renderer.table(&table)?;
        }
    }
    Ok(0)
}

/// Row number (1-based, as `list` prints) or a name; ambiguity is refused.
fn select<'a>(rows: &'a [DisplayInfo], target: &str) -> Result<&'a DisplayInfo> {
    if let Ok(number) = target.parse::<usize>() {
        if number == 0 {
            return Err(Error::invalid_input(
                "display numbers start at 1, as `x display list` prints them",
            ));
        }
        return rows.get(number - 1).ok_or_else(|| {
            Error::not_found(format!(
                "no display number {number}; the platform reports {} display(s)",
                rows.len()
            ))
        });
    }
    let needle = target.to_ascii_lowercase();
    if let Some(exact) = rows
        .iter()
        .find(|row| row.name.to_ascii_lowercase() == needle)
    {
        return Ok(exact);
    }
    let matches: Vec<&DisplayInfo> = rows
        .iter()
        .filter(|row| row.name.to_ascii_lowercase().contains(&needle))
        .collect();
    match matches.as_slice() {
        [] => Err(Error::not_found(format!("no display matching {target:?}"))),
        [one] => Ok(one),
        several => Err(Error::invalid_input(format!(
            "{target:?} matches {} displays: {}",
            several.len(),
            several
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

fn yes_no(value: Option<bool>) -> String {
    match value {
        Some(true) => "yes".to_string(),
        Some(false) => "no".to_string(),
        None => "-".to_string(),
    }
}

fn resolution(info: &DisplayInfo) -> String {
    match info.resolution {
        Some(size) => format!("{}x{}", size.width, size.height),
        None => "-".to_string(),
    }
}

fn refresh(info: &DisplayInfo) -> String {
    match info.refresh_hz {
        Some(hz) => format!("{hz}"),
        None => "-".to_string(),
    }
}

fn scale(info: &DisplayInfo) -> String {
    match info.scale_percent {
        Some(percent) => format!("{percent}%"),
        None => "-".to_string(),
    }
}

/// HDR state as three distinct answers.
///
/// `yes`/`no` means the platform said; `-` means it did not. Printing `no` for
/// a platform that has no HDR concept at all would be a claim nobody can check.
fn hdr(info: &DisplayInfo) -> String {
    match info.hdr_enabled {
        Some(true) => "yes".to_string(),
        Some(false) => "no".to_string(),
        None => "-".to_string(),
    }
}

fn position(info: &DisplayInfo) -> String {
    match info.position {
        Some(point) => format!("({},{})", point.x, point.y),
        None => "-".to_string(),
    }
}
