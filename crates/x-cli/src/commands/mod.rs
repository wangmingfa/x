//! One module per capability, mirroring the `x-core` manager traits.

pub mod bluetooth;
pub mod capability;
pub mod clipboard;
pub mod completion;
pub mod device;
pub mod devtools;
pub mod disk;
pub mod display;
pub mod envpath;
pub mod events;
pub mod file;
pub mod firewall;
pub mod gitcmd;
pub mod hosts;
pub mod logs;
pub mod manpage;
pub mod mount;
pub mod net;
pub mod netdiag;
pub mod permission;
pub mod port;
pub mod powertime;
pub mod proxy;
pub mod ps;
pub mod schedule;
pub mod service;
pub mod shell;
pub mod ssh;
pub mod startup;
pub mod sys;
pub mod usergroup;
pub mod window;

use x_core::error::Result;

use crate::format::Renderer;

/// Print adapter and contract versions, useful when reporting a bug.
pub fn version(renderer: &mut Renderer) -> Result<i32> {
    let info = serde_json::json!({
        "x": env!("CARGO_PKG_VERSION"),
        "contract": x_core::CONTRACT_VERSION,
        "platform": x_platform::platform_name(),
    });
    renderer.always_json(&info)?;
    Ok(0)
}

/// Dry-run gate for destructive commands.
///
/// Returns `Ok(true)` when `--dry-run` is on: the command has printed what it
/// *would* do and must stop without mutating anything. `Ok(false)` means
/// proceed. Read-only commands never call this.
pub fn dry_run_guard(renderer: &mut Renderer, action: &str) -> Result<bool> {
    if renderer.dry_run() {
        renderer.line(format!(
            "dry-run: would {action} (pass --yes and drop --dry-run to execute)"
        ))?;
        return Ok(true);
    }
    Ok(false)
}
