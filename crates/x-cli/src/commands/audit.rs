//! `x audit`: read back what this machine recorded about itself.
//!
//! Pure read, like `x logs`: no confirmation, no elevation, and no new audit
//! line (a query changes nothing, and a log that audits its own reads would
//! grow every time anyone looked at it).
//!
//! The log is JSON lines this project wrote, so the parsing and filtering live
//! in `x_core::audit` as pure functions; only the path is platform-specific.

use clap::Subcommand;
use x_core::audit::{AuditQuery, AuditReadReport};
use x_core::error::Result;

use crate::format::{OutputFormat, Renderer, Table};
use crate::row;

/// Default rows shown when `--limit` is not given.
const DEFAULT_LIMIT: usize = 50;

/// `x audit` subcommands.
#[derive(Debug, Subcommand)]
pub enum AuditCommand {
    /// Records of destructive actions, newest first.
    List {
        /// Only this action (`process.kill`), or a whole domain (`process.*`).
        #[arg(long)]
        action: Option<String>,

        /// Only records written by this user.
        #[arg(long)]
        user: Option<String>,

        /// Only records whose outcome was not `ok`.
        #[arg(long)]
        failures: bool,

        /// Only records at or after this RFC 3339 UTC stamp,
        /// e.g. `2026-10-01T00:00:00Z`.
        #[arg(long)]
        since: Option<String>,

        /// Free text over action, target and outcome.
        #[arg(long)]
        grep: Option<String>,

        /// Maximum number of rows.
        #[arg(long, short = 'n')]
        limit: Option<usize>,

        /// Where the log lives; defaults to the platform path.
        #[arg(long)]
        path: Option<String>,
    },
}

/// Route an `x audit` invocation.
pub fn dispatch(renderer: &mut Renderer, command: &AuditCommand) -> Result<i32> {
    match command {
        AuditCommand::List {
            action,
            user,
            failures,
            since,
            grep,
            limit,
            path,
        } => list(
            renderer,
            &AuditQuery {
                action: action.clone(),
                user: user.clone(),
                failures_only: *failures,
                since: since.clone(),
                grep: grep.clone(),
                // Applied after filtering, so `--limit 10 --failures` shows ten
                // failures rather than ten of everything.
                limit: Some(limit.unwrap_or(DEFAULT_LIMIT)),
            },
            path.as_deref(),
        ),
    }
}

fn list(renderer: &mut Renderer, query: &AuditQuery, path: Option<&str>) -> Result<i32> {
    let report = read(query, path);

    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&report)?;
        return Ok(0);
    }

    if report.entries.is_empty() {
        // An empty log and an over-filtered one are different situations, and
        // saying so is the difference between "nothing happened" and "your
        // filter matched nothing".
        if report.malformed_lines > 0 || report.truncated_last_line {
            renderer.line(format!(
                "no matching records ({} unreadable line(s), {} truncated tail)",
                report.malformed_lines,
                usize::from(report.truncated_last_line),
            ))?;
        } else {
            renderer.line("no audit records match")?;
        }
        return Ok(0);
    }

    let mut table = Table::new(["time", "user", "action", "target", "outcome"]);
    for entry in &report.entries {
        table.push(row![
            entry.time.clone(),
            entry.user.clone().unwrap_or_else(|| "-".into()),
            entry.action.clone(),
            entry.target.clone(),
            entry.outcome.clone(),
        ]);
    }
    renderer.table(&table)?;

    // A torn tail is worth surfacing: those records exist but were not parsed,
    // so a reader must not treat the list as complete.
    if report.truncated_last_line {
        renderer.line("note: the last record was truncated by an interrupted write")?;
    } else if report.malformed_lines > 0 {
        renderer.line(format!(
            "note: {} unreadable line(s) skipped",
            report.malformed_lines
        ))?;
    }
    Ok(0)
}

/// Read the log, honouring an explicit `--path` over the platform default.
///
/// Both branches do the same three things — parse, flip to newest-first, then
/// filter — and the filters must apply on either path: an explicit `--path`
/// is how a user reads a log copied off another machine, so ignoring
/// `--action` there would make the flag look broken.
fn read(query: &AuditQuery, path: Option<&str>) -> AuditReadReport {
    let text = match path {
        Some(explicit) => std::fs::read_to_string(explicit).ok(),
        None => x_platform::audit::read_text(),
    };
    let Some(text) = text else {
        return AuditReadReport::default();
    };

    let mut parsed = x_core::audit::parse_log(&text);
    // Append order is oldest first; everything downstream wants newest first.
    parsed.entries.reverse();
    let entries = x_core::audit::filter(parsed.entries, query);
    AuditReadReport {
        entries,
        malformed_lines: parsed.malformed_lines,
        truncated_last_line: parsed.truncated_last_line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::io::Write;
    use std::rc::Rc;

    /// Capturing sink, matching the renderer tests' shape.
    #[derive(Clone, Default)]
    struct Buffer(Rc<RefCell<Vec<u8>>>);

    impl Write for Buffer {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            self.0.borrow_mut().extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Buffer {
        fn text(&self) -> String {
            String::from_utf8(self.0.borrow().clone()).expect("utf8")
        }
    }

    fn line(action: &str, user: &str, outcome: &str, time: &str) -> String {
        format!(
            r#"{{"time":"{time}","user":"{user}","action":"{action}","target":"the {action} target","outcome":"{outcome}"}}"#
        )
    }

    /// A log written oldest first, the way the writer appends.
    fn log_file(name: &str, body: &str) -> String {
        let path = std::env::temp_dir().join(format!("x-audit-test-{name}"));
        std::fs::write(&path, body).expect("write log");
        path.to_string_lossy().into_owned()
    }

    fn render(name: &str, body: &str, query: &AuditQuery) -> String {
        let path = log_file(name, body);
        let buffer = Buffer::default();
        let mut renderer = Renderer::to_sink(OutputFormat::Plain, false, buffer.clone());
        list(&mut renderer, query, Some(&path)).expect("list");
        let _ = std::fs::remove_file(&path);
        buffer.text()
    }

    fn query() -> AuditQuery {
        AuditQuery::default()
    }

    #[test]
    fn records_are_listed_newest_first() {
        let body = format!(
            "{}\n{}\n",
            line("process.kill", "root", "ok", "2026-10-01T10:00:00Z"),
            line("service.action", "root", "ok", "2026-10-01T10:00:05Z"),
        );
        let out = render("order", &body, &query());
        let older = out.find("process.kill").expect("kill listed");
        let newer = out.find("service.action").expect("service listed");
        assert!(newer < older, "the later record comes first:\n{out}");
    }

    #[test]
    fn a_limit_keeps_the_most_recent_records() {
        let body = format!(
            "{}\n{}\n{}\n",
            line("process.kill", "root", "ok", "2026-10-01T10:00:00Z"),
            line("process.kill", "root", "ok", "2026-10-01T10:00:01Z"),
            line("process.kill", "root", "ok", "2026-10-01T10:00:02Z"),
        );
        let limited = AuditQuery {
            limit: Some(2),
            ..query()
        };
        let out = render("limit", &body, &limited);
        // The plain format is one tab separated line per row, no header.
        assert_eq!(out.lines().count(), 2, "two rows:\n{out}");
        assert!(out.contains("10:00:02Z"), "the newest survives:\n{out}");
        assert!(out.contains("10:00:01Z"), "then the one before:\n{out}");
        assert!(!out.contains("10:00:00Z"), "the oldest is dropped:\n{out}");
    }

    #[test]
    fn a_filter_that_matches_nothing_says_so() {
        let body = format!(
            "{}\n",
            line("process.kill", "root", "ok", "2026-10-01T10:00:00Z")
        );
        let filtered = AuditQuery {
            action: Some("service.*".into()),
            ..query()
        };
        let out = render("nomatch", &body, &filtered);
        assert!(out.contains("no audit records match"), "{out}");
    }

    #[test]
    fn a_missing_log_is_reported_as_no_records_not_as_a_failure() {
        // Auditing may simply never have fired; that is not an error.
        let buffer = Buffer::default();
        let mut renderer = Renderer::to_sink(OutputFormat::Plain, false, buffer.clone());
        let code = list(&mut renderer, &query(), Some("no/such/audit/log")).expect("list");
        assert_eq!(code, 0);
        assert!(buffer.text().contains("no audit records match"));
    }

    #[test]
    fn a_torn_tail_is_surfaced_rather_than_hidden() {
        // Those records exist but were not parsed, so a reader must not treat
        // the listing as complete.
        let mut body = format!(
            "{}\n",
            line("process.kill", "root", "ok", "2026-10-01T10:00:00Z")
        );
        body.push_str(r#"{"time":"2026-10-01T10:00:01Z","action":"service.ac"#);
        let out = render("torn", &body, &query());
        assert!(out.contains("truncated by an interrupted write"), "{out}");
        assert!(out.contains("process.kill"), "{out}");
    }

    #[test]
    fn unreadable_lines_are_counted_in_the_summary() {
        let body = format!(
            "{}\nnot json\n{}\n",
            line("process.kill", "root", "ok", "2026-10-01T10:00:00Z"),
            line("service.action", "root", "ok", "2026-10-01T10:00:01Z"),
        );
        let out = render("garbage", &body, &query());
        assert!(out.contains("1 unreadable line(s) skipped"), "{out}");
    }

    #[test]
    fn filters_apply_to_an_explicit_path_too() {
        // `--path` is how a log copied off another machine gets read, so the
        // filters must not be silently dropped on that route.
        let body = format!(
            "{}\n{}\n",
            line("process.kill", "root", "ok", "2026-10-01T10:00:00Z"),
            line("service.action", "root", "ok", "2026-10-01T10:00:01Z"),
        );
        let filtered = AuditQuery {
            action: Some("service.*".into()),
            ..query()
        };
        let out = render("pathfilter", &body, &filtered);
        assert!(out.contains("service.action"), "{out}");
        assert!(!out.contains("process.kill"), "{out}");
    }

    #[test]
    fn failures_only_hides_successful_records() {
        let body = format!(
            "{}\n{}\n",
            line("process.kill", "root", "ok", "2026-10-01T10:00:00Z"),
            line(
                "process.kill",
                "root",
                "error: permission denied",
                "2026-10-01T10:00:01Z"
            ),
        );
        let failed = AuditQuery {
            failures_only: true,
            ..query()
        };
        let out = render("failures", &body, &failed);
        assert!(out.contains("permission denied"), "{out}");
        // One row: the plain format prints no header.
        assert_eq!(out.lines().count(), 1, "only the failure:\n{out}");
    }
}
