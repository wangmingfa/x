//! One module per capability, mirroring the `x-core` manager traits.

pub mod capability;
pub mod clipboard;
pub mod devtools;
pub mod disk;
pub mod envpath;
pub mod file;
pub mod firewall;
pub mod gitcmd;
pub mod hosts;
pub mod mount;
pub mod net;
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
