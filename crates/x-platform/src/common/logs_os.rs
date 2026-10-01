//! `x logs`: read the platform's own system log store.
//!
//! journald (`-o json`), the Windows Event Log (`Get-WinEvent`) and the macOS
//! unified log (`log show --style json`) all answer through their native tool
//! and hand back structured text, so the parsing stays honest — no hand rolled
//! text scraping of localised tables. Levels and timestamps are passed through
//! in the platform's own words, like [`x_core::service::ServiceLogEntry`].
//!
//! Read-only everywhere: no confirmation, no elevation, no audit line. A
//! permission problem with the store itself (journald access groups) surfaces
//! as the tool's own error text.

use x_core::error::{Error, Result};
use x_core::logs::{LogEntry, LogPage, LogReader, LogScope};

/// Platform log reader plugged into [`x_core::context::SystemContext`].
pub struct PlatformLogs;

impl LogReader for PlatformLogs {
    fn read(&self, scope: &LogScope, limit: usize) -> Result<LogPage> {
        #[cfg(target_os = "linux")]
        {
            linux_read(scope, limit)
        }
        #[cfg(target_os = "macos")]
        {
            macos_read(scope, limit)
        }
        #[cfg(windows)]
        {
            windows_read(scope, limit)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        {
            let _ = (scope, limit);
            Err(Error::unsupported("no system log reader for this platform"))
        }
    }
}

/// Names a selector may carry: no control characters, not empty.
fn checked_name(raw: &str) -> Result<String> {
    let name = raw.trim();
    if name.is_empty() {
        return Err(Error::invalid_input("log selector must not be empty"));
    }
    if name.chars().any(|c| c.is_control()) {
        return Err(Error::invalid_input(format!(
            "log selector `{raw}` contains control characters"
        )));
    }
    Ok(name.to_string())
}

/// A selector that reaches a script shell must additionally survive quoting.
#[cfg(windows)]
fn checked_script_name(raw: &str) -> Result<String> {
    let name = checked_name(raw)?;
    Ok(name.replace('\'', "''"))
}

// ---------------------------------------------------------------------------
// Linux: journalctl
// ---------------------------------------------------------------------------

/// Extra journalctl arguments for one scope.
#[cfg(target_os = "linux")]
fn journal_args(scope: &LogScope) -> Result<Vec<String>> {
    Ok(match scope {
        LogScope::System => Vec::new(),
        LogScope::Service(name) => vec!["-u".to_string(), checked_name(name)?],
        LogScope::Process(target) => {
            let target = checked_name(target)?;
            if target.bytes().all(|b| b.is_ascii_digit()) {
                vec![format!("_PID={target}")]
            } else {
                vec![format!("_COMM={target}")]
            }
        }
    })
}

#[cfg(target_os = "linux")]
fn linux_read(scope: &LogScope, limit: usize) -> Result<LogPage> {
    let mut args = vec![
        "--no-pager".to_string(),
        "-o".to_string(),
        "json".to_string(),
        "-n".to_string(),
        limit.to_string(),
    ];
    args.extend(journal_args(scope)?);
    let text = run_program("journalctl", &args)?;
    Ok(LogPage {
        scope: scope.label(),
        source: "journalctl".to_string(),
        entries: parse_journal_json(&text),
    })
}

/// Parse `journalctl -o json`: one JSON object per line.
#[cfg(target_os = "linux")]
fn parse_journal_json(text: &str) -> Vec<LogEntry> {
    text.lines()
        .map(str::trim)
        .filter(|line| line.starts_with('{'))
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .map(journal_entry)
        .collect()
}

#[cfg(target_os = "linux")]
fn journal_entry(record: serde_json::Value) -> LogEntry {
    let timestamp = record
        .get("__REALTIME_TIMESTAMP")
        .and_then(|v| v.as_str())
        .map(|micros| match micros.parse::<i64>() {
            Ok(value) => x_core::rfc3339_utc(value / 1_000_000),
            Err(_) => micros.to_string(),
        });
    let level =
        record
            .get("PRIORITY")
            .and_then(|v| v.as_str())
            .map(|prio| match prio.parse::<u8>() {
                Ok(number) if number <= 7 => [
                    "emerg", "alert", "crit", "err", "warning", "notice", "info", "debug",
                ][usize::from(number)]
                .to_string(),
                _ => prio.to_string(),
            });
    let origin = ["_SYSTEMD_UNIT", "SYSLOG_IDENTIFIER", "COMM"]
        .iter()
        .find_map(|key| record.get(*key).and_then(|v| v.as_str()))
        .map(str::to_string);
    // MESSAGE is a string for UTF-8 payloads; binary messages arrive as a
    // JSON object, which is passed through as its own serialisation.
    let message = match record.get("MESSAGE") {
        Some(serde_json::Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    };
    LogEntry {
        timestamp,
        level,
        origin,
        message,
    }
}

// ---------------------------------------------------------------------------
// macOS: log show
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn log_show_args(scope: &LogScope) -> Result<Vec<String>> {
    let mut args = vec![
        "show".to_string(),
        "--style".to_string(),
        "json".to_string(),
        "--last".to_string(),
        "1h".to_string(),
    ];
    match scope {
        LogScope::System => {}
        LogScope::Service(name) | LogScope::Process(name) => {
            let name = checked_name(name)?;
            let predicate = if name.bytes().all(|b| b.is_ascii_digit()) {
                format!("processid == {name}")
            } else {
                format!("process == \"{name}\"")
            };
            args.extend(["--predicate".to_string(), predicate]);
        }
    }
    Ok(args)
}

#[cfg(target_os = "macos")]
fn macos_read(scope: &LogScope, limit: usize) -> Result<LogPage> {
    let args = log_show_args(scope)?;
    let text = run_program("log", &args)?;
    let mut entries = parse_unified_log_json(&text);
    // `log show` prints oldest first; the page contract is newest first.
    entries.reverse();
    entries.truncate(limit);
    Ok(LogPage {
        scope: scope.label(),
        source: "log show".to_string(),
        entries,
    })
}

/// Parse a `log show --style json` array.
///
/// Field spelling has drifted across macOS releases, so each field is looked
/// up under several key names before it is given up on.
#[cfg(target_os = "macos")]
fn parse_unified_log_json(text: &str) -> Vec<LogEntry> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text.trim()) else {
        return Vec::new();
    };
    let records: Vec<&serde_json::Value> = match &value {
        serde_json::Value::Array(rows) => rows.iter().collect(),
        serde_json::Value::Object(_) => vec![&value],
        _ => Vec::new(),
    };
    records
        .iter()
        .map(|record| LogEntry {
            timestamp: pick_str(record, &["Timestamp", "Time", "timestamp"]),
            level: pick_str(record, &["Level", "level", "Type", "type"]),
            origin: pick_str(record, &["Process", "process", "processImagePath"]),
            message: record
                .get("message")
                .or_else(|| record.get("Message"))
                .map(|m| match m {
                    serde_json::Value::String(text) => text.clone(),
                    other => other.to_string(),
                })
                .unwrap_or_default(),
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn pick_str(record: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        record
            .get(*key)
            .and_then(|v| v.as_str())
            .filter(|text| !text.is_empty())
            .map(str::to_string)
    })
}

// ---------------------------------------------------------------------------
// Windows: Get-WinEvent
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn windows_read(scope: &LogScope, limit: usize) -> Result<LogPage> {
    let script = match scope {
        LogScope::System => {
            format!("Get-WinEvent -LogName System -MaxEvents {limit} -ErrorAction SilentlyContinue")
        }
        LogScope::Service(name) => {
            let name = checked_script_name(name)?;
            format!(
                "Get-WinEvent -FilterHashtable @{{ProviderName='{name}'}} \
                 -MaxEvents {limit} -ErrorAction SilentlyContinue"
            )
        }
        LogScope::Process(_) => {
            return Err(Error::unsupported(
                "the Windows Event Log is not indexed per process; \
                 try `x logs service <provider>` or `x service logs <name>`",
            ))
        }
    };
    let full = format!(
        "{script} | Select-Object \
         @{{N='Time';E={{$_.TimeCreated.ToString('o')}}}},@{{N='Level';E={{$_.LevelDisplayName}}}},\
         @{{N='Provider';E={{$_.ProviderName}}}},@{{N='Message';E={{$_.Message}}}} \
         | ConvertTo-Json -Compress"
    );
    let text = run_powershell(&full)?;
    Ok(LogPage {
        scope: scope.label(),
        source: "Get-WinEvent".to_string(),
        entries: parse_winevent_json(&text),
    })
}

/// Parse `ConvertTo-Json` output: an array, or a lone object for one event.
#[cfg(windows)]
fn parse_winevent_json(text: &str) -> Vec<LogEntry> {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed == "null" {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
        return Vec::new();
    };
    let records: Vec<&serde_json::Value> = match &value {
        serde_json::Value::Array(rows) => rows.iter().collect(),
        serde_json::Value::Object(_) => vec![&value],
        _ => Vec::new(),
    };
    records
        .iter()
        .map(|record| {
            let text_of = |key: &str| record.get(key).and_then(|v| v.as_str()).map(str::to_string);
            LogEntry {
                timestamp: text_of("Time"),
                level: text_of("Level"),
                origin: text_of("Provider"),
                message: text_of("Message").unwrap_or_default(),
            }
        })
        .collect()
}

#[cfg(windows)]
fn run_powershell(script: &str) -> Result<String> {
    let (code, stdout, stderr) = crate::sys::run_command_capture(
        "powershell.exe",
        &["-NoProfile", "-NonInteractive", "-Command", script],
    )
    .map_err(|e| Error::system(format!("cannot run powershell.exe: {e}")))?;
    if code == 0 {
        return Ok(crate::windows::service::decode_console(&stdout));
    }
    let stderr = crate::windows::service::decode_console(&stderr);
    let stderr = stderr.trim();
    Err(Error::system(if stderr.is_empty() {
        format!("powershell.exe failed (exit {code})")
    } else {
        format!("powershell.exe failed: {stderr}")
    }))
}

// ---------------------------------------------------------------------------
// Shared runner (unix family)
// ---------------------------------------------------------------------------

/// Run a log tool, taking stdout on success and stderr as the error text.
#[cfg(not(windows))]
fn run_program(program: &str, args: &[String]) -> Result<String> {
    let output = std::process::Command::new(program)
        .args(args)
        .output()
        .map_err(|e| Error::system(format!("cannot run {program}: {e}")))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(Error::system(format!(
        "{program} failed: {}",
        if stderr.is_empty() {
            "non-zero exit"
        } else {
            &stderr
        }
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_name_rejects_blanks_and_controls() {
        assert_eq!(checked_name(" nginx ").expect("ok"), "nginx");
        assert!(checked_name("").is_err());
        assert!(checked_name("web\nservice").is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn journal_lines_become_normalised_entries() {
        let text = concat!(
            "{\"__REALTIME_TIMESTAMP\":\"1700000000000000\",\"PRIORITY\":\"3\",",
            "\"_SYSTEMD_UNIT\":\"nginx.service\",\"MESSAGE\":\"worker exited\"}\n",
            "No journal files were found.\n",
            "{\"__REALTIME_TIMESTAMP\":\"1700000001000000\",\"PRIORITY\":\"7\",",
            "\"COMM\":\"cron\",\"MESSAGE\":{\"op\":\"hex\"}}\n",
        );
        let entries = parse_journal_json(text);
        assert_eq!(entries.len(), 2, "noise lines are skipped");
        assert_eq!(
            entries[0].timestamp.as_deref(),
            Some("2023-11-14T22:13:20Z")
        );
        assert_eq!(entries[0].level.as_deref(), Some("err"));
        assert_eq!(entries[0].origin.as_deref(), Some("nginx.service"));
        assert_eq!(entries[0].message, "worker exited");
        // Binary messages keep their serialisation; the origin falls back to COMM.
        assert_eq!(entries[1].level.as_deref(), Some("debug"));
        assert_eq!(entries[1].origin.as_deref(), Some("cron"));
        assert_eq!(entries[1].message, "{\"op\":\"hex\"}");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn journal_scopes_map_to_their_own_fields() {
        assert!(journal_args(&LogScope::System).expect("ok").is_empty());
        assert_eq!(
            journal_args(&LogScope::Service("ssh".into())).expect("ok"),
            ["-u", "ssh"]
        );
        assert_eq!(
            journal_args(&LogScope::Process("4242".into())).expect("ok"),
            ["_PID=4242"]
        );
        assert_eq!(
            journal_args(&LogScope::Process("nginx".into())).expect("ok"),
            ["_COMM=nginx"]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unified_log_records_survive_key_drift() {
        let text = r#"[{"Time":"2026-10-01 12:00:00","Level":"Error","Process":"nginx","message":"boom"},{"Timestamp":"t2","Type":"Default","process":"ssh","message":42}]"#;
        let entries = parse_unified_log_json(text);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].timestamp.as_deref(), Some("2026-10-01 12:00:00"));
        assert_eq!(entries[0].level.as_deref(), Some("Error"));
        assert_eq!(entries[0].origin.as_deref(), Some("nginx"));
        assert_eq!(entries[0].message, "boom");
        assert_eq!(entries[1].level.as_deref(), Some("Default"));
        assert_eq!(entries[1].message, "42");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn unified_log_scopes_become_predicates() {
        let args = log_show_args(&LogScope::Process("4242".into())).expect("ok");
        assert!(args.contains(&"processid == 4242".to_string()));
        let args = log_show_args(&LogScope::Service("nginx".into())).expect("ok");
        assert!(args.contains(&"process == \"nginx\"".to_string()));
    }

    #[cfg(windows)]
    #[test]
    fn winevent_json_accepts_object_array_and_empties() {
        let one = parse_winevent_json(
            r#"{"Time":"t","Level":"错误","Provider":"Service Control Manager","Message":"stopped"}"#,
        );
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].level.as_deref(), Some("错误"));
        let many = parse_winevent_json(r#"[{"Message":"a"},{"Message":"b"}]"#);
        assert_eq!(many.len(), 2);
        assert!(parse_winevent_json("").is_empty());
        assert!(parse_winevent_json("null").is_empty());
        assert!(parse_winevent_json("not json").is_empty());
    }

    #[cfg(windows)]
    #[test]
    fn provider_names_are_quote_escaped() {
        assert_eq!(checked_script_name("Hyper-V'").expect("ok"), "Hyper-V''");
    }
}
