//! Login shells.
//!
//! "Which shell am I in", "which shells exist" and "which one do I get at
//! login" are three different questions on every OS; the trait keeps them
//! separate instead of overloading one field.

use crate::error::Result;
use serde::{Deserialize, Serialize};

/// A shell the platform knows about.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ShellInfo {
    /// Short name (`bash`, `zsh`, `pwsh`, `cmd`).
    pub name: String,
    /// Executable path when it could be located.
    pub path: Option<String>,
    /// Version string when the shell can report one cheaply.
    pub version: Option<String>,
}

/// Shell inspection.
pub trait ShellManager: Send + Sync {
    /// The shell this process is running under, when detectable.
    fn current(&self) -> Result<ShellInfo>;

    /// Shells installed on this machine.
    fn list(&self) -> Result<Vec<ShellInfo>>;

    /// The shell new login sessions get by default.
    fn default(&self) -> Result<ShellInfo>;
}
