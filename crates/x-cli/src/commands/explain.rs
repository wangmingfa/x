//! `x explain`: what would this command do?
//!
//! A pure, offline explainer — no AI involved, which is the roadmap's explicit
//! stance. It walks the parsed clap grammar along the words the user passed,
//! prints what every level means, lists the flags it recognised, and flags
//! destructive verbs with an explicit warning. The result is generated from
//! the same tree `x --help` and the man pages come from, so it cannot drift.

use clap::CommandFactory;
use x_core::error::Result;

use crate::format::Renderer;

/// Verbs whose effect is not read-only; explaining them earns a warning.
const DESTRUCTIVE_HINTS: &[(&str, &str)] = &[
    (
        "kill",
        "terminates processes — work in progress can be lost",
    ),
    (
        "trash",
        "moves the path to the platform trash / recycle bin",
    ),
    ("shutdown", "powers the machine off"),
    ("reboot", "restarts the machine"),
    ("sleep", "suspends the machine"),
    (
        "disable",
        "turns the item off (startup item, service, rule …)",
    ),
    ("enable", "turns the item on"),
    ("remove", "deletes the entry"),
    ("unmount", "detaches the filesystem; open files will fail"),
    ("deny", "blocks traffic at the firewall"),
    ("allow", "opens a hole in the firewall"),
    ("clear", "erases the contents (clipboard, proxy env …)"),
    ("connect", "opens a live session to a remote host"),
    ("set", "changes configuration (this process or the OS)"),
];

/// `x explain <words…>` arguments: the command to explain, verbatim.
#[derive(Debug, clap::Args)]
pub struct ExplainArgs {
    /// The command to explain, e.g. `port kill 8080` (without the leading `x`).
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
    pub words: Vec<String>,
}

/// Route an `x explain` invocation.
pub fn dispatch(
    _context: &x_core::SystemContext,
    renderer: &mut Renderer,
    args: &ExplainArgs,
) -> Result<i32> {
    let mut command = crate::Cli::command();
    let mut path: Vec<String> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();

    for word in &args.words {
        let next = command
            .get_subcommands()
            .find(|sub| sub.get_name() == word || sub.get_aliases().any(|alias| alias == word));
        match next {
            Some(sub) => {
                path.push(sub.get_name().to_string());
                command = sub.clone();
            }
            None => unknown.push(word.clone()),
        }
    }

    renderer.line(format!("x {}", path.join(" ")))?;
    if let Some(about) = command.get_about() {
        renderer.line(format!("  {about}"))?;
    }
    if let Some(long) = command.get_long_about() {
        for paragraph in long.to_string().lines().filter(|l| !l.trim().is_empty()) {
            renderer.line(format!("  {paragraph}"))?;
        }
    }

    // Positional / required arguments the command expects.
    let required: Vec<String> = command
        .get_positionals()
        .filter(|arg| arg.is_required_set())
        .map(|arg| format!("<{}>", arg.get_id()))
        .collect();
    if !required.is_empty() {
        renderer.line(format!("  needs: {}", required.join(" ")))?;
    }

    // Flags the user's words referenced that the grammar knows about.
    let known_flags: Vec<&str> = args
        .words
        .iter()
        .filter(|w| w.starts_with('-'))
        .filter(|w| {
            command.get_arguments().any(|arg| {
                arg.get_long() == Some(w.trim_start_matches("--")) || arg.get_id() == w.as_str()
            })
        })
        .map(|w| w.as_str())
        .collect();
    if !known_flags.is_empty() {
        renderer.line(format!("  flags: {}", known_flags.join(" ")))?;
    }

    // Subcommands still available below this point.
    let subs: Vec<&str> = command.get_subcommands().map(|s| s.get_name()).collect();
    if !subs.is_empty() {
        renderer.line(format!("  subcommands: {}", subs.join(", ")))?;
    }

    // Destructive warnings for every verb on the path.
    let mut warned = false;
    for verb in &path {
        for (hint_verb, warning) in DESTRUCTIVE_HINTS {
            if verb == hint_verb {
                renderer.line(format!("  ⚠ destructive: {warning}"))?;
                warned = true;
            }
        }
    }
    if !warned {
        renderer.line("  read-only: nothing on this path changes the machine")?;
    }

    if !unknown.is_empty() {
        renderer.line(format!(
            "  note: not recognised as a subcommand here: {} (treated as arguments)",
            unknown.join(" ")
        ))?;
    }
    Ok(0)
}
