//! Audit records for destructive operations.
//!
//! `kill`, port reclaiming and service lifecycle change the machine, so `x`
//! writes a local trail an operator can read back later. This module is the
//! *shape* of one record plus the UTC timestamp it carries; where the record
//! goes and when it is written is platform policy (`x-platform::audit`).
//!
//! One record per line, JSON, so the file stays `grep`-able for humans and
//! `jq`-able for scripts without pulling in a database.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// One audited operation and how it went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// UTC wall clock, RFC 3339 (`2026-10-01T12:34:56Z`).
    pub time: String,
    /// User the operation ran as, when the platform can name it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// What was done, in `domain.verb` form (`process.kill`, `service.action`).
    pub action: String,
    /// What it was done to (`pid 4242 (terminate)`, `Spooler (stop)`).
    pub target: String,
    /// `ok`, or `error: …` / a result summary for partial failures.
    pub outcome: String,
}

impl AuditEntry {
    /// The whole record on one line, newline included: the format the audit
    /// log appends.
    ///
    /// Serialising this shape cannot fail (every field is a string), but the
    /// audit path must never panic, so a degenerate line still carries the
    /// action if serde were ever to refuse.
    pub fn json_line(&self) -> String {
        let fallback = format!(
            "{{\"action\":\"{}\",\"target\":\"{}\"}}",
            self.action.replace('"', "\\\""),
            self.target.replace('"', "\\\""),
        );
        let mut line = serde_json::to_string(self).unwrap_or(fallback);
        line.push('\n');
        line
    }
}

/// Current UTC time as RFC 3339 with second precision.
pub fn now_utc_rfc3339() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default();
    rfc3339_utc(seconds)
}

/// UNIX seconds → `YYYY-MM-DDThh:mm:ssZ`, without a date crate.
///
/// The civil-date conversion is the days-from-epoch algorithm by Howard
/// Hinnant; it handles leap years, centuries and timestamps before 1970.
pub fn rfc3339_utc(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let second_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        second_of_day / 3600,
        (second_of_day % 3600) / 60,
        second_of_day % 60,
    )
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097; // [0, 146096]
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// Filters for reading the audit log back.
///
/// The log is JSON lines written by this same crate, so reading it needs no
/// platform adapter: the only platform-specific part is the path, which the
/// writer already resolved.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditQuery {
    /// Keep only this action (`process.kill`), or any action in the
    /// `domain` when written as `process.*`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// Keep only records written by this user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// Keep only records whose outcome is not `ok`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub failures_only: bool,
    /// Keep only records at or after this RFC 3339 UTC stamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    /// Free text over action, target and outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grep: Option<String>,
    /// Keep at most this many records, newest first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Whether `entry` satisfies every filter in `query`.
///
/// `--action process.*` is the one glob: `x audit list --action process.kill`
/// and `--action process.*` are both things a user reaches for, and the log is
/// small enough that matching is trivial either way.
pub fn matches(entry: &AuditEntry, query: &AuditQuery) -> bool {
    if let Some(action) = &query.action {
        let wanted = action.trim();
        let hit = if let Some(domain) = wanted.strip_suffix(".*") {
            entry.action.starts_with(domain)
                && entry.action.as_bytes().get(domain.len()) == Some(&b'.')
        } else {
            entry.action.eq_ignore_ascii_case(wanted)
        };
        if !hit {
            return false;
        }
    }
    if let Some(user) = &query.user {
        let hit = entry
            .user
            .as_deref()
            .is_some_and(|name| name.eq_ignore_ascii_case(user.trim()));
        if !hit {
            return false;
        }
    }
    if query.failures_only && entry.outcome.trim() == "ok" {
        return false;
    }
    // RFC 3339 UTC sorts lexicographically, so no date parsing is needed; the
    // comparison is deliberately string-based and exact rather than a lenient
    // "parse and hope" that would silently drop records.
    if let Some(since) = &query.since {
        if entry.time.as_str() < since.as_str() {
            return false;
        }
    }
    if let Some(needle) = &query.grep {
        let needle = needle.to_ascii_lowercase();
        let haystack =
            format!("{} {} {}", entry.action, entry.target, entry.outcome).to_ascii_lowercase();
        if !haystack.contains(&needle) {
            return false;
        }
    }
    true
}

/// Apply `query` to records given newest first, keeping their order.
pub fn filter(entries: Vec<AuditEntry>, query: &AuditQuery) -> Vec<AuditEntry> {
    let mut kept: Vec<AuditEntry> = entries.into_iter().filter(|e| matches(e, query)).collect();
    if let Some(limit) = query.limit {
        kept.truncate(limit);
    }
    kept
}

/// How much of the log could not be read.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditReadReport {
    /// Records parsed and kept after filtering.
    pub entries: Vec<AuditEntry>,
    /// Lines that were not valid audit JSON. A half-written final line is
    /// normal after a crash mid-append, so this is counted, not fatal.
    pub malformed_lines: usize,
    /// Lines the writer never finished writing.
    pub truncated_last_line: bool,
}

impl AuditReadReport {
    /// `true` when nothing usable came back.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Parse JSON-lines audit text into records, newest first.
///
/// A line that does not parse is counted and skipped: the writer appends
/// without locking, so a torn line from a killed process must not make the
/// whole log unreadable. A record missing its mandatory fields is skipped the
/// same way, because a half-record would render as an empty row.
pub fn parse_log(text: &str) -> AuditReadReport {
    let mut report = AuditReadReport::default();
    let mut lines: Vec<&str> = text.lines().collect();
    let ended_mid_line = !text.ends_with('\n') && !text.is_empty();
    if ended_mid_line {
        // The last line was never terminated, so it was being written when the
        // previous run stopped.
        report.truncated_last_line = true;
        lines.pop();
    }

    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<AuditEntry>(trimmed) {
            Ok(entry) if !entry.action.is_empty() => report.entries.push(entry),
            _ => report.malformed_lines += 1,
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unix_epoch_is_the_first_january_of_1970() {
        assert_eq!(rfc3339_utc(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn known_timestamps_format_exactly() {
        assert_eq!(rfc3339_utc(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(rfc3339_utc(2_000_000_000), "2033-05-18T03:33:20Z");
    }

    #[test]
    fn leap_days_and_centuries_are_counted() {
        // 29 February 2000 (a century year that is a leap year).
        assert_eq!(rfc3339_utc(951_782_400), "2000-02-29T00:00:00Z");
        // 2024-12-31T23:59:59Z, the last second before a 366-day year ends.
        assert_eq!(rfc3339_utc(1_735_689_599), "2024-12-31T23:59:59Z");
    }

    #[test]
    fn timestamps_before_1970_use_the_pre_epoch_second() {
        assert_eq!(rfc3339_utc(-1), "1969-12-31T23:59:59Z");
    }

    #[test]
    fn now_is_a_utc_rfc3339_stamp() {
        let now = now_utc_rfc3339();
        assert_eq!(now.len(), 20, "{now}");
        assert!(now.ends_with('Z'));
        assert_eq!(now.as_bytes()[4], b'-');
    }

    #[test]
    fn a_record_is_one_json_line_and_drops_an_absent_user() {
        let entry = AuditEntry {
            time: "2026-10-01T00:00:00Z".into(),
            user: None,
            action: "process.kill".into(),
            target: "pid 42 (terminate)".into(),
            outcome: "ok".into(),
        };
        let line = entry.json_line();
        assert!(line.ends_with("}\n"), "{line}");
        assert!(line.contains("\"action\":\"process.kill\""));
        assert!(!line.contains("user"));
        assert!(!line[..line.len() - 1].contains('\n'));

        let with_user = AuditEntry {
            user: Some("alice".into()),
            ..entry
        };
        assert!(with_user.json_line().contains("\"user\":\"alice\""));
    }

    #[test]
    fn a_record_round_trips_through_its_own_line() {
        let entry = AuditEntry {
            time: now_utc_rfc3339(),
            user: Some("root".into()),
            action: "service.native".into(),
            target: "sc query Spooler".into(),
            outcome: "ok".into(),
        };
        let parsed: AuditEntry = serde_json::from_str(&entry.json_line()).expect("parse");
        assert_eq!(parsed, entry);
    }

    fn entry(action: &str, user: &str, outcome: &str, time: &str) -> AuditEntry {
        AuditEntry {
            time: time.into(),
            user: Some(user.into()),
            action: action.into(),
            target: format!("target for {action}"),
            outcome: outcome.into(),
        }
    }

    fn query() -> AuditQuery {
        AuditQuery::default()
    }

    // --- reading the log back ----------------------------------------------

    #[test]
    fn parsing_reads_every_valid_record() {
        let text = concat!(
            r#"{"time":"2026-10-01T10:00:00Z","user":"root","action":"process.kill","target":"pid 42","outcome":"ok"}"#,
            "\n",
            r#"{"time":"2026-10-01T10:00:01Z","user":"root","action":"service.action","target":"sshd (stop)","outcome":"ok"}"#,
            "\n",
        );
        let report = parse_log(text);
        assert_eq!(report.entries.len(), 2);
        assert_eq!(report.malformed_lines, 0);
        assert!(!report.truncated_last_line);
    }

    #[test]
    fn a_torn_final_line_is_counted_not_fatal() {
        // The writer appends without a lock, so a process killed mid-append
        // leaves half a line. It must not make the whole log unreadable.
        let text = concat!(
            r#"{"time":"2026-10-01T10:00:00Z","user":"root","action":"process.kill","target":"pid 42","outcome":"ok"}"#,
            "\n",
            r#"{"time":"2026-10-01T10:00:01Z","action":"service.ac"#,
        );
        let report = parse_log(text);
        assert_eq!(report.entries.len(), 1, "the good record survives");
        assert!(report.truncated_last_line);
        assert_eq!(report.malformed_lines, 0, "a torn tail is not corruption");
    }

    #[test]
    fn genuinely_malformed_lines_are_counted_and_skipped() {
        let text = concat!(
            "not json at all\n",
            "\n",
            r#"{"time":"2026-10-01T10:00:00Z","action":"process.kill","target":"pid 1","outcome":"ok"}"#,
            "\n",
            r#"{"no":"action field"}"#,
            "\n",
        );
        let report = parse_log(text);
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.malformed_lines, 2, "garbage and actionless");
    }

    #[test]
    fn an_empty_log_parses_to_nothing_without_error() {
        let report = parse_log("");
        assert!(report.is_empty());
        assert_eq!(report.malformed_lines, 0);
        assert!(!report.truncated_last_line);
    }

    #[test]
    fn an_action_filter_matches_exactly_or_by_domain() {
        let kill = entry("process.kill", "root", "ok", "2026-10-01T10:00:00Z");
        let kill_many = entry("process.kill_many", "root", "ok", "2026-10-01T10:00:01Z");
        let service = entry("service.action", "root", "ok", "2026-10-01T10:00:02Z");

        let exact = AuditQuery {
            action: Some("process.kill".into()),
            ..query()
        };
        assert!(matches(&kill, &exact));
        assert!(!matches(&kill_many, &exact), "kill_many is not kill");

        let domain = AuditQuery {
            action: Some("process.*".into()),
            ..query()
        };
        assert!(matches(&kill, &domain));
        assert!(matches(&kill_many, &domain));
        assert!(!matches(&service, &domain));
    }

    #[test]
    fn a_domain_glob_does_not_match_a_domain_with_the_same_prefix() {
        // `process` must not pull in `processional.*`.
        let decoy = entry("processional.kill", "root", "ok", "2026-10-01T10:00:00Z");
        let domain = AuditQuery {
            action: Some("process.*".into()),
            ..query()
        };
        assert!(!matches(&decoy, &domain));
    }

    #[test]
    fn a_user_filter_is_case_insensitive_and_needs_a_name() {
        let root = entry("process.kill", "ROOT", "ok", "2026-10-01T10:00:00Z");
        let anonymous = AuditEntry {
            time: "2026-10-01T10:00:01Z".into(),
            user: None,
            action: "process.kill".into(),
            target: "pid 1".into(),
            outcome: "ok".into(),
        };

        let wanted = AuditQuery {
            user: Some("root".into()),
            ..query()
        };
        assert!(matches(&root, &wanted));
        assert!(!matches(&anonymous, &wanted), "no user, no match");
    }

    #[test]
    fn failures_only_keeps_outcomes_that_are_not_ok() {
        let ok = entry("process.kill", "root", "ok", "2026-10-01T10:00:00Z");
        let refused = entry(
            "process.kill",
            "root",
            "error: permission denied",
            "2026-10-01T10:00:01Z",
        );

        let failures = AuditQuery {
            failures_only: true,
            ..query()
        };
        assert!(!matches(&ok, &failures));
        assert!(matches(&refused, &failures));
    }

    #[test]
    fn since_compares_stamps_as_text_because_rfc3339_sorts() {
        let early = entry("process.kill", "root", "ok", "2026-10-01T09:59:59Z");
        let late = entry("process.kill", "root", "ok", "2026-10-01T10:00:01Z");

        let since = AuditQuery {
            since: Some("2026-10-01T10:00:00Z".into()),
            ..query()
        };
        assert!(!matches(&early, &since));
        assert!(matches(&late, &since), "the boundary is inclusive");
    }

    #[test]
    fn grep_searches_action_target_and_outcome() {
        let row = entry("process.kill", "root", "ok", "2026-10-01T10:00:00Z");

        for needle in ["kill", "TARGET FOR", "ok"] {
            let found = AuditQuery {
                grep: Some(needle.into()),
                ..query()
            };
            assert!(matches(&row, &found), "{needle} should match");
        }
        let missing = AuditQuery {
            grep: Some("nothing-here".into()),
            ..query()
        };
        assert!(!matches(&row, &missing));
    }

    #[test]
    fn every_filter_must_pass_together() {
        let rows = vec![
            entry("process.kill", "root", "ok", "2026-10-01T10:00:00Z"),
            entry(
                "process.kill",
                "root",
                "error: denied",
                "2026-10-01T10:00:01Z",
            ),
            entry(
                "service.action",
                "root",
                "error: denied",
                "2026-10-01T10:00:02Z",
            ),
        ];

        let combined = AuditQuery {
            action: Some("process.*".into()),
            failures_only: true,
            ..query()
        };
        let kept: Vec<String> = filter(rows, &combined)
            .into_iter()
            .map(|e| e.outcome)
            .collect();
        assert_eq!(kept, vec!["error: denied"], "only the process failure");
    }

    #[test]
    fn a_limit_keeps_the_newest_records() {
        // The reader hands records over newest first, so a plain truncate
        // keeps the newest and never reorders.
        let rows = vec![
            entry("process.kill", "root", "ok", "2026-10-01T10:00:02Z"),
            entry("process.kill", "root", "ok", "2026-10-01T10:00:01Z"),
            entry("process.kill", "root", "ok", "2026-10-01T10:00:00Z"),
        ];
        let limited = AuditQuery {
            limit: Some(2),
            ..query()
        };
        let kept = filter(rows, &limited);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].time, "2026-10-01T10:00:02Z");
    }

    #[test]
    fn an_empty_query_keeps_everything() {
        let rows = vec![
            entry("process.kill", "root", "ok", "2026-10-01T10:00:00Z"),
            entry(
                "service.action",
                "alice",
                "error: nope",
                "2026-10-01T10:00:01Z",
            ),
        ];
        assert_eq!(filter(rows, &query()).len(), 2);
    }
}
