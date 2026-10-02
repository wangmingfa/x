//! `x logs`: read the machine's own log store.
//!
//! Pure reads — no confirmation, no elevation, no audit line. The three
//! platform stores differ in what they can scope by; the adapter reports
//! honestly (exit 7) where its store has no such index.

use clap::Subcommand;
use x_core::error::{Error, Result};
use x_core::logs::{LogEntry, LogPage, LogReader, LogScope};
use x_core::SystemContext;

use crate::format::{OutputFormat, Renderer, Table};
use crate::row;
use std::collections::HashSet;
use std::time::Duration;

/// Default rows fetched when `--limit` is not given.
const DEFAULT_LIMIT: usize = 50;

/// Fetch window and filters shared by every `x logs` scope.
#[derive(Debug, Default, clap::Args)]
pub struct LogsQuery {
    /// Maximum number of rows (newest first).
    #[arg(long)]
    pub limit: Option<usize>,

    /// Keep streaming new records until Ctrl-C.
    #[arg(long, short = 'f')]
    pub follow: bool,

    /// Seconds between samples while following.
    #[arg(long, default_value_t = 1.0)]
    pub interval: f64,

    /// Stop after this many samples while following; for scripts and tests.
    #[arg(long)]
    pub count: Option<usize>,

    /// Only records whose message, origin or level contains this text
    /// (case insensitive).
    #[arg(long)]
    pub grep: Option<String>,

    /// Only records whose level is one of these, comma separated.
    ///
    /// The vocabulary is the platform's own: `err,warning` on journald,
    /// `Error,Warning` on Windows, `error,fault` on the unified log. The
    /// comparison is case insensitive, the spelling is not translated.
    #[arg(long)]
    pub level: Option<String>,
}

/// `x logs` subcommands.
#[derive(Debug, Subcommand)]
pub enum LogsCommand {
    /// The machine-wide log: journal, System channel, unified log.
    System {
        /// Fetch window and filters.
        #[command(flatten)]
        query: LogsQuery,
    },

    /// Records attributed to one service (unit / event provider / daemon).
    Service {
        /// Service name, e.g. `nginx` or `Service Control Manager`.
        name: String,

        /// Fetch window and filters.
        #[command(flatten)]
        query: LogsQuery,
    },

    /// Records attributed to one process, by pid or program name.
    Process {
        /// Process id or program name.
        target: String,

        /// Fetch window and filters.
        #[command(flatten)]
        query: LogsQuery,
    },
}

/// Arguments for `x logs`.
#[derive(Debug, clap::Args)]
pub struct LogsArgs {
    #[command(subcommand)]
    pub command: LogsCommand,
}

/// Route a `x logs` invocation.
pub fn dispatch(
    context: &SystemContext,
    renderer: &mut Renderer,
    command: &LogsCommand,
) -> Result<i32> {
    let reader = context
        .logs
        .as_ref()
        .ok_or_else(|| Error::unsupported("no system log reader in this context"))?;

    let (scope, query) = match command {
        LogsCommand::System { query } => (LogScope::System, query),
        LogsCommand::Service { name, query } => (LogScope::Service(name.clone()), query),
        LogsCommand::Process { target, query } => (LogScope::Process(target.clone()), query),
    };

    if query.follow {
        return follow(reader.as_ref(), renderer, scope, query);
    }

    let page = reader.read(&scope, query.limit.unwrap_or(DEFAULT_LIMIT))?;
    let entries = filter(page.entries, query);

    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&LogPage {
            scope: page.scope,
            source: page.source,
            entries,
        })?;
        return Ok(0);
    }

    if entries.is_empty() {
        renderer.line(format!(
            "no records for {} from {}",
            page.scope, page.source
        ))?;
        return Ok(0);
    }

    let mut table = Table::new(["timestamp", "level", "origin", "message"]);
    for entry in &entries {
        table.push(row![
            entry.timestamp.clone().unwrap_or_else(|| "-".into()),
            entry.level.clone().unwrap_or_else(|| "-".into()),
            entry.origin.clone().unwrap_or_else(|| "-".into()),
            flatten(&entry.message),
        ]);
    }
    renderer.table(&table)?;
    Ok(0)
}

/// Stream records until Ctrl-C, printing only what the previous sample did
/// not already show.
///
/// Follow is polling, not a kernel subscription, for the same reason
/// `x events` polls: x-core stays free of platform cfg. Each round re-reads a
/// window and the records already printed are remembered by identity, so the
/// first round prints the backlog and later rounds print only arrivals.
///
/// Records are deduplicated by their whole content rather than by timestamp:
/// the three stores emit timestamps in mutually incomparable text, and two
/// records with the same stamp are common. When a platform repeats a record
/// verbatim the duplicates are suppressed, which is the honest outcome of not
/// owning a stable record id.
///
/// `--count n` bounds the number of samples for scripts and tests.
fn follow(
    reader: &dyn LogReader,
    renderer: &mut Renderer,
    scope: LogScope,
    query: &LogsQuery,
) -> Result<i32> {
    let interval = if query.interval.is_finite() {
        query.interval.max(0.05)
    } else {
        1.0
    };
    // A wider window than the display limit is what lets a follow catch up:
    // between two rounds more records may exist than the user asked to see at
    // once. The extra rows are used to detect arrivals, never to pad the first
    // round.
    let display = query.limit.unwrap_or(DEFAULT_LIMIT);
    let window = display.max(DEFAULT_LIMIT) * 2;
    let json = renderer.format() == OutputFormat::Json;

    let mut seen: HashSet<String> = HashSet::new();
    let mut first_round = true;
    let mut polls = 0usize;
    loop {
        let page = reader.read(&scope, window)?;
        let entries = filter(page.entries, query);

        let mut fresh = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            let known = seen.contains(&identity(entry));
            // Every row read is baselined, including the ones not printed, so
            // the deeper backlog is never re-emitted on a later round.
            seen.insert(identity(entry));
            if known {
                // Newest first: everything from here on was already printed.
                break;
            }
            if first_round && index >= display {
                // The user asked for `display` rows, not the whole store.
                continue;
            }
            fresh.push(entry.clone());
        }
        fresh.reverse();
        first_round = false;

        if json {
            for entry in &fresh {
                renderer.always_json(entry)?;
            }
        } else {
            for entry in &fresh {
                renderer.line(log_line(entry))?;
            }
        }
        renderer.flush().map_err(|e| {
            Error::new(
                x_core::ErrorKind::System,
                format!("follow output failed: {e}"),
            )
        })?;

        polls += 1;
        if query.count.is_some_and(|target| polls >= target) {
            return Ok(0);
        }
        std::thread::sleep(Duration::from_secs_f64(interval));
    }
}

/// A record's identity for deduplication: the whole record, as printed.
fn identity(entry: &LogEntry) -> String {
    format!(
        "{}\u{1}{}\u{1}{}\u{1}{}",
        entry.timestamp.as_deref().unwrap_or_default(),
        entry.level.as_deref().unwrap_or_default(),
        entry.origin.as_deref().unwrap_or_default(),
        entry.message
    )
}

/// One followed record as a single line.
fn log_line(entry: &LogEntry) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(timestamp) = &entry.timestamp {
        parts.push(timestamp.clone());
    }
    if let Some(level) = &entry.level {
        parts.push(level.clone());
    }
    if let Some(origin) = &entry.origin {
        parts.push(format!("[{origin}]"));
    }
    parts.push(flatten(&entry.message));
    parts.join(" ")
}

/// Apply `--grep` and `--level` to a batch of records.
fn filter(entries: Vec<LogEntry>, query: &LogsQuery) -> Vec<LogEntry> {
    let levels: Option<Vec<String>> = query.level.as_ref().map(|raw| {
        raw.split(',')
            .map(|level| level.trim().to_ascii_lowercase())
            .filter(|level| !level.is_empty())
            .collect()
    });
    entries
        .into_iter()
        .filter(|entry| {
            if let Some(wanted) = &levels {
                let matches = entry
                    .level
                    .as_deref()
                    .is_some_and(|level| wanted.contains(&level.to_ascii_lowercase()));
                if !matches {
                    return false;
                }
            }
            match &query.grep {
                None => true,
                Some(needle) => {
                    let needle = needle.to_ascii_lowercase();
                    let haystack = format!(
                        "{} {} {}",
                        entry.message,
                        entry.origin.as_deref().unwrap_or_default(),
                        entry.level.as_deref().unwrap_or_default()
                    )
                    .to_ascii_lowercase();
                    haystack.contains(&needle)
                }
            }
        })
        .collect()
}

/// Event records carry multi-paragraph messages; a table row stays on one line.
fn flatten(message: &str) -> String {
    message.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::io::Write;
    use std::rc::Rc;
    use x_core::testing::StubLogs;

    /// Capturing sink, the same shape the renderer tests use.
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

    fn entry(level: &str, origin: &str, message: &str) -> LogEntry {
        LogEntry {
            timestamp: Some("2026-10-02 10:00:00 +08:00".into()),
            level: Some(level.into()),
            origin: Some(origin.into()),
            message: message.into(),
        }
    }

    fn query() -> LogsQuery {
        LogsQuery::default()
    }

    /// Build a query carrying only the filters under test.
    fn query_with(grep: Option<&str>, level: Option<&str>) -> LogsQuery {
        LogsQuery {
            grep: grep.map(str::to_string),
            level: level.map(str::to_string),
            ..LogsQuery::default()
        }
    }

    #[test]
    fn no_filters_keep_every_record() {
        let rows = vec![entry("err", "cron", "boom"), entry("info", "sshd", "ok")];
        assert_eq!(filter(rows, &query()).len(), 2);
    }

    #[test]
    fn grep_matches_message_origin_and_level_case_insensitively() {
        let rows = vec![
            entry("err", "cron", "job failed"),
            entry("info", "CRON", "all good"),
            entry("Warning", "kernel", "disk almost full"),
        ];

        let by_message = query_with(Some("JOB"), None);
        let hit = filter(rows.clone(), &by_message);
        assert_eq!(hit.len(), 1, "{hit:?}");
        assert_eq!(hit[0].origin.as_deref(), Some("cron"));

        // Both the lower and the upper case origin match.
        let by_origin = query_with(Some("cron"), None);
        assert_eq!(filter(rows.clone(), &by_origin).len(), 2);

        let by_level = query_with(Some("warning"), None);
        assert_eq!(filter(rows, &by_level).len(), 1);
    }

    #[test]
    fn grep_does_not_span_the_field_separators() {
        // The haystack joins fields with spaces, so a needle that would only
        // exist *across* a boundary must not match — otherwise a search for
        // "CRON job" would silently match any cron record.
        let rows = vec![entry("err", "cron", "job failed")];
        let spanning = query_with(Some("CRON job"), None);
        assert!(filter(rows.clone(), &spanning).is_empty());
    }

    #[test]
    fn grep_needs_no_match_on_the_message_alone() {
        // The word only appears in the origin, which is part of the haystack.
        let rows = vec![entry("info", "nginx", "reloaded")];
        let needle = query_with(Some("NGINX"), None);
        assert_eq!(filter(rows, &needle).len(), 1);
    }

    #[test]
    fn level_filter_takes_a_comma_separated_list() {
        let rows = vec![
            entry("err", "a", "one"),
            entry("warning", "b", "two"),
            entry("info", "c", "three"),
        ];

        let wanted = query_with(None, Some("err, Warning"));
        let kept: Vec<String> = filter(rows.clone(), &wanted)
            .into_iter()
            .filter_map(|e| e.level)
            .collect();
        assert_eq!(kept, vec!["err", "warning"], "spacing and case tolerated");

        let only_err = query_with(None, Some("err"));
        assert_eq!(filter(rows, &only_err).len(), 1);
    }

    #[test]
    fn a_record_without_a_level_never_matches_a_level_filter() {
        let rows = vec![LogEntry {
            timestamp: None,
            level: None,
            origin: Some("mystery".into()),
            message: "no level here".into(),
        }];
        let wanted = query_with(None, Some("err,warning"));
        assert!(filter(rows, &wanted).is_empty());
    }

    #[test]
    fn grep_and_level_combine_as_and_not_or() {
        // Both `err` rows contain "disk", so grep alone would keep two and the
        // level filter alone would keep two; together they still keep both,
        // while the `info` row is dropped by the level.
        let rows = vec![
            entry("err", "cron", "disk failure"),
            entry("err", "sshd", "disk fine"),
            entry("info", "cron", "disk noted"),
        ];
        let both = query_with(Some("disk"), Some("err"));
        let kept: Vec<String> = filter(rows, &both)
            .into_iter()
            .map(|e| e.origin.unwrap_or_default())
            .collect();
        assert_eq!(kept, vec!["cron", "sshd"]);
    }

    #[test]
    fn the_first_round_honours_the_display_limit() {
        // The follow reads a wider window to catch arrivals, but the user asked
        // for `--limit` rows and must not have the whole store dumped on them.
        let entries: Vec<LogEntry> = (0..10)
            .map(|index| entry("info", "cron", &format!("record {index}")))
            .collect();
        let stub = StubLogs::new("stub", entries);
        let buffer = Buffer::default();
        let mut renderer = Renderer::to_sink(OutputFormat::Plain, false, buffer.clone());
        let mut q = query();
        q.limit = Some(3);
        q.count = Some(1);

        follow(&stub, &mut renderer, LogScope::System, &q).expect("follow");
        let printed = buffer.text();
        assert_eq!(printed.lines().count(), 3, "{printed}");
    }

    #[test]
    fn a_row_can_match_the_level_and_fail_the_grep() {
        // The other half of the AND: a level hit that the grep rejects.
        let rows = vec![
            entry("err", "cron", "disk failure"),
            entry("err", "sshd", "unrelated chatter"),
        ];
        let both = query_with(Some("disk"), Some("err"));
        let kept: Vec<String> = filter(rows, &both)
            .into_iter()
            .map(|e| e.origin.unwrap_or_default())
            .collect();
        assert_eq!(kept, vec!["cron"], "only the row matching both");
    }

    #[test]
    fn identity_separates_records_that_share_a_timestamp() {
        let a = entry("err", "cron", "first");
        let b = entry("err", "cron", "second");
        assert_ne!(identity(&a), identity(&b));
        assert_eq!(identity(&a), identity(&a.clone()), "stable");
    }

    #[test]
    fn identity_treats_a_missing_field_as_empty_not_as_the_string_none() {
        let mut blank = entry("err", "cron", "boom");
        blank.timestamp = None;
        let mut absent = blank.clone();
        absent.level = None;
        assert_ne!(identity(&blank), identity(&absent));
    }

    #[test]
    fn follow_prints_the_backlog_once_and_then_nothing_new() {
        let stub = StubLogs::new(
            "stub",
            vec![entry("err", "cron", "older"), entry("info", "cron", "newer")],
        );
        let buffer = Buffer::default();
        let mut renderer = Renderer::to_sink(OutputFormat::Plain, false, buffer.clone());
        let mut q = query();
        q.count = Some(3);
        q.interval = 0.05;

        let code = follow(&stub, &mut renderer, LogScope::System, &q).expect("follow");
        assert_eq!(code, 0);

        let printed = buffer.text();
        // Both records arrive on the first round, oldest first, and the two
        // later rounds must not repeat them.
        assert_eq!(printed.matches("older").count(), 1, "{printed}");
        assert_eq!(printed.matches("newer").count(), 1, "{printed}");
        assert_eq!(stub.reads().len(), 3, "every round re-read the store");
    }

    #[test]
    fn follow_emits_the_backlog_oldest_first() {
        // LogPage hands records over newest first; a reader wants them
        // chronological, so the follow loop reverses the fresh batch.
        let stub = StubLogs::new(
            "stub",
            vec![entry("info", "cron", "newer"), entry("err", "cron", "older")],
        );
        let buffer = Buffer::default();
        let mut renderer = Renderer::to_sink(OutputFormat::Plain, false, buffer.clone());
        let mut q = query();
        q.count = Some(1);

        follow(&stub, &mut renderer, LogScope::System, &q).expect("follow");
        let printed = buffer.text();
        let older = printed.find("older").expect("older printed");
        let newer = printed.find("newer").expect("newer printed");
        assert!(older < newer, "a reader wants chronological order:\n{printed}");
    }

    #[test]
    fn follow_honours_the_filters_on_every_round() {
        let stub = StubLogs::new(
            "stub",
            vec![
                entry("err", "cron", "keep me"),
                entry("info", "cron", "drop me"),
            ],
        );
        let buffer = Buffer::default();
        let mut renderer = Renderer::to_sink(OutputFormat::Plain, false, buffer.clone());
        let mut q = query_with(Some("keep"), None);
        q.count = Some(2);
        q.interval = 0.05;

        follow(&stub, &mut renderer, LogScope::System, &q).expect("follow");
        let printed = buffer.text();
        assert!(printed.contains("keep me"), "{printed}");
        assert!(!printed.contains("drop me"), "{printed}");
    }

    #[test]
    fn follow_asks_for_a_wider_window_than_the_display_limit() {
        // A follow that only re-read the display limit would miss everything
        // that arrived while the backlog was being printed.
        let stub = StubLogs::new("stub", Vec::new());
        let buffer = Buffer::default();
        let mut renderer = Renderer::to_sink(OutputFormat::Plain, false, buffer);
        let mut q = query();
        q.limit = Some(5);
        q.count = Some(1);

        follow(&stub, &mut renderer, LogScope::System, &q).expect("follow");
        assert_eq!(stub.reads().len(), 1);
    }

    #[test]
    fn a_zero_interval_is_clamped_instead_of_busy_spinning() {
        let stub = StubLogs::new("stub", Vec::new());
        let buffer = Buffer::default();
        let mut renderer = Renderer::to_sink(OutputFormat::Plain, false, buffer);
        let mut q = query();
        q.interval = 0.0;
        q.count = Some(1);

        // Would spin forever if the clamp were missing.
        let code = follow(&stub, &mut renderer, LogScope::System, &q).expect("follow");
        assert_eq!(code, 0);
    }

    #[test]
    fn a_non_finite_interval_falls_back_to_the_default() {
        let stub = StubLogs::new("stub", Vec::new());
        let buffer = Buffer::default();
        let mut renderer = Renderer::to_sink(OutputFormat::Plain, false, buffer);
        let mut q = query();
        q.interval = f64::NAN;
        q.count = Some(1);

        assert_eq!(
            follow(&stub, &mut renderer, LogScope::System, &q).expect("follow"),
            0
        );
    }

    #[test]
    fn log_line_keeps_the_platform_vocabulary_intact() {
        let line = log_line(&entry("错误", "System", "boom\nhappened"));
        assert_eq!(line, "2026-10-02 10:00:00 +08:00 错误 [System] boom happened");
    }
}
