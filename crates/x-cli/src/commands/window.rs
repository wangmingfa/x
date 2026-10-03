//! `x window`: the desktop's windows, and the three control verbs.
//!
//! Reads (`list`, `active`) pass through untouched — fields a platform does
//! not report are rendered as `-` and omitted from JSON. `focus` / `minimize`
//! / `maximize` change state, so they resolve the target from a fresh list,
//! confirm, and are audited by the platform decorator.

use clap::Subcommand;
use x_core::error::{Error, Result};
use x_core::window::WindowInfo;
use x_core::SystemContext;

use crate::{
    format::{Confirmer, OutputFormat, Renderer, Table},
    row,
};

/// `x window` subcommands.
#[derive(Debug, Subcommand)]
pub enum WindowCommand {
    /// Every window the platform reports.
    List,

    /// The window that currently holds the focus.
    Active,

    /// Raise and focus a window.
    Focus(TargetArgs),

    /// Minimize a window.
    Minimize(TargetArgs),

    /// Maximize a window.
    Maximize(TargetArgs),
}

/// Arguments for the window verbs.
#[derive(Debug, clap::Args)]
pub struct TargetArgs {
    /// Row number from `x window list` (1-based), or a title fragment.
    pub target: String,
    /// Do not ask for confirmation.
    #[arg(long, short = 'y')]
    pub yes: bool,
}

/// Arguments for `x window`.
#[derive(Debug, clap::Args)]
pub struct WindowArgs {
    #[command(subcommand)]
    pub command: WindowCommand,
}

/// Route a `x window` invocation.
pub fn dispatch(
    context: &SystemContext,
    renderer: &mut Renderer,
    confirmer: &mut dyn Confirmer,
    command: &WindowCommand,
) -> Result<i32> {
    let windows = context
        .window
        .as_ref()
        .ok_or_else(|| Error::unsupported("no window capability in this context"))?;

    match command {
        WindowCommand::List => render_list(renderer, &windows.windows()?)?,
        WindowCommand::Active => render_active(renderer, windows.active()?.as_ref())?,
        WindowCommand::Focus(args) => {
            return verb(renderer, confirmer, windows.as_ref(), args, "focus");
        }
        WindowCommand::Minimize(args) => {
            return verb(renderer, confirmer, windows.as_ref(), args, "minimize");
        }
        WindowCommand::Maximize(args) => {
            return verb(renderer, confirmer, windows.as_ref(), args, "maximize");
        }
    }
    Ok(0)
}

/// Resolve, confirm, run one control verb. The audit decorator records the
/// outcome — including a refusal by the platform.
fn verb(
    renderer: &mut Renderer,
    confirmer: &mut dyn Confirmer,
    windows: &dyn x_core::window::WindowManager,
    args: &TargetArgs,
    action: &str,
) -> Result<i32> {
    let rows = windows.windows()?;
    let target = select(&rows, &args.target)?;
    let label = target.label();
    if crate::dry_run_guard(renderer, &format!("{action} {label}"))? {
        return Ok(0);
    }
    // `--yes` skips the question; the output format never does.
    if !super::confirm(renderer, confirmer, args.yes, &format!("{action} {label}"))? {
        return Ok(super::EXIT_DECLINED);
    }
    match action {
        "focus" => windows.focus(target)?,
        "minimize" => windows.minimize(target)?,
        _ => windows.maximize(target)?,
    }
    renderer.line(format!(
        "{}ed {label}",
        match action {
            "focus" => "focus",
            "minimize" => "minimiz",
            _ => "maximiz",
        }
    ))?;
    Ok(0)
}

/// The list table; the number column is the selection key for the verbs.
fn render_list(renderer: &mut Renderer, rows: &[WindowInfo]) -> Result<()> {
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&rows)?;
        return Ok(());
    }
    if rows.is_empty() {
        renderer.line("no windows reported by the platform")?;
        return Ok(());
    }
    let mut table = Table::new([
        "#",
        "title",
        "app",
        "pid",
        "active",
        "minimized",
        "maximized",
    ]);
    for (index, info) in rows.iter().enumerate() {
        table.push(row![
            (index + 1).to_string(),
            info.title.clone().unwrap_or_else(|| "-".into()),
            info.app.clone().unwrap_or_else(|| "-".into()),
            info.pid
                .map(|pid| pid.to_string())
                .unwrap_or_else(|| "-".to_string()),
            yes_no(info.active),
            yes_no(info.minimized),
            yes_no(info.maximized),
        ]);
    }
    renderer.table(&table)?;
    Ok(())
}

/// `active` as a vertical view of the one focused window (or nothing).
fn render_active(renderer: &mut Renderer, info: Option<&WindowInfo>) -> Result<()> {
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&info)?;
        return Ok(());
    }
    let Some(info) = info else {
        renderer.line("no active window")?;
        return Ok(());
    };
    let mut table = Table::new(["field", "value"]);
    table.push(row![
        "title",
        info.title.clone().unwrap_or_else(|| "-".into())
    ]);
    table.push(row!["id", info.id.clone().unwrap_or_else(|| "-".into())]);
    table.push(row!["app", info.app.clone().unwrap_or_else(|| "-".into())]);
    table.push(row![
        "pid",
        info.pid
            .map(|pid| pid.to_string())
            .unwrap_or_else(|| "-".to_string())
    ]);
    table.push(row!["active", yes_no(info.active)]);
    table.push(row!["minimized", yes_no(info.minimized)]);
    table.push(row!["maximized", yes_no(info.maximized)]);
    renderer.table(&table)?;
    Ok(())
}

/// Row number (1-based, as `list` prints) or a title fragment; ambiguity is
/// refused, an exact title match wins over a substring one.
fn select<'a>(rows: &'a [WindowInfo], target: &str) -> Result<&'a WindowInfo> {
    if let Ok(number) = target.parse::<usize>() {
        if number == 0 {
            return Err(Error::invalid_input(
                "window numbers start at 1, as `x window list` prints them",
            ));
        }
        return rows.get(number - 1).ok_or_else(|| {
            Error::not_found(format!(
                "no window number {number}; the platform reports {} window(s)",
                rows.len()
            ))
        });
    }
    let needle = target.to_ascii_lowercase();
    let title_of = |row: &WindowInfo| row.title.clone().unwrap_or_default().to_ascii_lowercase();
    if let Some(exact) = rows.iter().find(|row| title_of(row) == needle) {
        return Ok(exact);
    }
    let matches: Vec<&WindowInfo> = rows
        .iter()
        .filter(|row| title_of(row).contains(&needle))
        .collect();
    match matches.as_slice() {
        [] => Err(Error::not_found(format!("no window matching {target:?}"))),
        [one] => Ok(one),
        several => Err(Error::invalid_input(format!(
            "{target:?} matches {} windows: {}",
            several.len(),
            several
                .iter()
                .map(|row| row.label())
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

/// Honest three-state rendering: a platform that does not report a flag
/// shows `-`, not `no`.
fn yes_no(value: Option<bool>) -> String {
    match value {
        Some(true) => "yes".to_string(),
        Some(false) => "no".to_string(),
        None => "-".to_string(),
    }
}
