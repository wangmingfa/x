//! Clipboard access with one vocabulary across platforms.
//!
//! Every platform ships a command line tool that can do this (`pbcopy` /
//! `pbpaste`, `wl-copy` / `wl-paste` or `xclip`, PowerShell's clipboard
//! cmdlets), so the adapters stay tiny; the trait just hides which one runs.

use crate::error::Result;

/// Read / write / clear the system clipboard.
pub trait ClipboardManager: Send + Sync {
    /// Current clipboard text. An empty clipboard reads as an empty string.
    fn get(&self) -> Result<String>;

    /// Replace the clipboard contents with `text`.
    fn set(&self, text: &str) -> Result<()>;

    /// Empty the clipboard.
    fn clear(&self) -> Result<()>;
}
