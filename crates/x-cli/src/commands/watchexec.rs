//! The `--exec` trigger shared by the `watch` commands.
//!
//! Running a command because a watched condition fired *changes the machine*,
//! so it goes through the same audit trail as kills and service actions. The
//! outcome is reported honestly: a failing command is printed with its exit
//! status and turns into exit code 1, never silently swallowed — a trigger
//! that did nothing is worse than no trigger.

use x_core::error::{Error, Result};

use crate::commands::port::WatchWhenArg;
use crate::format::Renderer;

/// Run `command` once for a fired watch condition, audit it, and report.
///
/// Returns the process exit code: 0 when the command succeeded, 1 when it
/// failed or could not be started.
pub fn run_triggered(
    renderer: &mut Renderer,
    command: &str,
    domain: &str,
    when: Option<WatchWhenArg>,
    summary: &str,
) -> Result<i32> {
    let condition = when.map_or("any".to_string(), |w| w.label().to_string());
    renderer.line(format!(
        "trigger: {domain} watch ({condition}) fired: {summary}"
    ))?;
    renderer.line(format!("exec: {command}"))?;
    renderer.flush()?;

    let (program, argument) = if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("sh", "-c")
    };
    let outcome = std::process::Command::new(program)
        .arg(argument)
        .arg(command)
        .output();

    let entry = x_core::audit::AuditEntry {
        time: x_core::audit::now_utc_rfc3339(),
        user: None,
        action: format!("{domain}.watch_exec"),
        target: command.to_string(),
        outcome: String::new(), // filled below
    };
    let (exit_code, outcome) = match outcome {
        Ok(output) if output.status.success() => (0, "ok".to_string()),
        Ok(output) => {
            let status = output
                .status
                .code()
                .map_or("signal".to_string(), |c| c.to_string());
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stderr = stderr.trim();
            if stderr.is_empty() {
                (1, format!("error: exit {status}"))
            } else {
                (1, format!("error: exit {status}: {stderr}"))
            }
        }
        Err(e) => (1, format!("error: cannot start: {e}")),
    };
    let mut entry = entry;
    entry.outcome = outcome;
    // The audit write itself must never break the trail's caller.
    let _ = x_platform::audit::record(&entry);

    if exit_code != 0 {
        renderer.line(format!("trigger command failed: {}", entry.outcome))?;
        renderer.flush()?;
        return Err(Error::new(
            x_core::ErrorKind::System,
            format!("watch --exec command failed: {}", entry.outcome),
        ));
    }
    renderer.line("trigger command ok")?;
    renderer.flush()?;
    Ok(0)
}
