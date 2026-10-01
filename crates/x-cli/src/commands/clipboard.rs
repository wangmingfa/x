//! `x clipboard`: read, write and clear the system clipboard.

use clap::Subcommand;
use x_core::error::{Error, Result};
use x_core::SystemContext;

use crate::format::Renderer;

/// `x clipboard` subcommands.
#[derive(Debug, Subcommand)]
pub enum ClipboardCommand {
    /// Print the current clipboard text.
    Get,

    /// Replace the clipboard with a value.
    Set {
        /// Text to put on the clipboard. Reads stdin when `--stdin` is set.
        value: Option<String>,
        /// Read the text from stdin instead of an argument.
        #[arg(long)]
        stdin: bool,
    },

    /// Empty the clipboard.
    Clear,
}

/// Arguments for `x clipboard`.
#[derive(Debug, clap::Args)]
pub struct ClipboardArgs {
    #[command(subcommand)]
    pub command: ClipboardCommand,
}

/// Route a `x clipboard` invocation.
pub fn dispatch(
    context: &SystemContext,
    renderer: &mut Renderer,
    command: &ClipboardCommand,
) -> Result<i32> {
    let clipboard = context
        .clipboard
        .as_ref()
        .ok_or_else(|| Error::unsupported("clipboard is not available in this context"))?;

    match command {
        ClipboardCommand::Get => {
            let text = clipboard.get()?;
            renderer.line(text)?;
        }
        ClipboardCommand::Set { value, stdin } => {
            let text = if *stdin {
                use std::io::Read;
                let mut buffer = String::new();
                std::io::stdin()
                    .read_to_string(&mut buffer)
                    .map_err(|e| Error::invalid_input(format!("cannot read stdin: {e}")))?;
                buffer
            } else {
                value
                    .clone()
                    .ok_or_else(|| Error::invalid_input("pass a value or use --stdin"))?
            };
            clipboard.set(&text)?;
            renderer.line(format!("clipboard set ({} chars)", text.chars().count()))?;
        }
        ClipboardCommand::Clear => {
            clipboard.clear()?;
            renderer.line("clipboard cleared")?;
        }
    }
    Ok(0)
}

/// Re-exported so the table helpers stay in one place for this module.
#[allow(unused_imports)]
use crate::format::OutputFormat;
