//! One module per capability, mirroring the `x-core` manager traits.

pub mod capability;
pub mod disk;
pub mod net;
pub mod port;
pub mod ps;
pub mod service;
pub mod sys;

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
