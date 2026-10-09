//! One module per capability, mirroring the `x-core` manager traits.

pub mod audit;
pub mod bench;
pub mod bluetooth;
pub mod capability;
pub mod clipboard;
pub mod completion;
pub mod configcmd;
pub mod device;
pub mod devtools;
pub mod disk;
pub mod display;
pub mod envpath;
pub mod events;
pub mod explain;
pub mod file;
pub mod firewall;
pub mod gitcmd;
pub mod hosts;
pub mod logs;
pub mod manpage;
pub mod mcp;
pub mod mount;
pub mod net;
pub mod netdiag;
pub mod permission;
pub mod plugins;
pub mod port;
pub mod powertime;
pub mod proxy;
pub mod ps;
pub mod remote;
pub mod schedule;
pub mod service;
pub mod shell;
pub mod ssh;
pub mod startup;
pub mod sys;
pub mod upgrade;
pub mod usergroup;
pub mod window;

use x_core::error::Result;

use crate::format::{Confirmer, OutputFormat, Renderer};

/// Print adapter and contract versions, useful when reporting a bug.
pub fn version(renderer: &mut Renderer) -> Result<i32> {
    let info = serde_json::json!({
        "x": env!("CARGO_PKG_VERSION"),
        "contract": x_core::CONTRACT_VERSION,
        "contract_doc": "docs/contract.md",
        "platform": x_platform::platform_name(),
    });
    renderer.always_json(&info)?;
    Ok(0)
}

/// Exit code for a destructive action the user declined.
pub const EXIT_DECLINED: i32 = 130;

/// Dry-run gate for destructive commands.
///
/// Returns `Ok(true)` when `--dry-run` is on: the command has printed what it
/// *would* do and must stop without mutating anything. `Ok(false)` means
/// proceed. Read-only commands never call this.
///
/// The gate runs BEFORE any confirmation, without exception: `--dry-run` is a
/// question about the future, not an action, so a piped
/// `x --dry-run ps kill 42` must print its plan and exit 0 instead of being
/// refused by a confirmation it can never answer.
pub fn dry_run_guard(renderer: &mut Renderer, action: &str) -> Result<bool> {
    if renderer.dry_run() {
        if renderer.format() == OutputFormat::Json {
            // A JSON script gets one document, not prose in its stream.
            renderer.always_json(&serde_json::json!({ "dry_run": true, "would": action }))?;
        } else {
            renderer.line(format!(
                "dry-run: would {action} (pass --yes and drop --dry-run to execute)"
            ))?;
        }
        return Ok(true);
    }
    Ok(false)
}

/// Ask for confirmation under the unified destructive-action contract.
///
/// - `--yes` skips the question entirely.
/// - The output format never skips the question: JSON is a format, not a
///   permission. Scripts driving destructive commands pass `--yes`; a
///   non-interactive stdin (a pipe) always answers no, so a script that
///   forgot `--yes` is refused, never surprises its author.
/// - A decline prints `aborted` (text mode) and the caller returns
///   [`EXIT_DECLINED`]. Kill commands with a JSON report shape emit their own
///   `aborted: true` document instead of the bare line.
///
/// Returns `Ok(true)` to proceed. Callers run the [`dry_run_guard`] first, so
/// `--dry-run` never reaches a question.
pub(crate) fn confirm(
    renderer: &mut Renderer,
    confirmer: &mut dyn Confirmer,
    yes: bool,
    action: &str,
) -> Result<bool> {
    if yes {
        return Ok(true);
    }
    if renderer.format() != OutputFormat::Json {
        renderer.line(format!("about to {action}"))?;
    }
    if confirmer.confirm("continue?")? {
        return Ok(true);
    }
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&serde_json::json!({ "aborted": true, "would": action }))?;
    } else {
        renderer.line("aborted")?;
    }
    Ok(false)
}
