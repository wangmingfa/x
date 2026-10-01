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
}
