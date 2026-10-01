//! The hosts file: parse, query and edit `/etc/hosts` (or the Windows
//! equivalent) with a path handed in by the platform layer.
//!
//! Everything here is pure text work — the file format is identical on all
//! three OSes, only the location differs. Writing the file usually needs
//! elevation; the CLI surfaces the platform's permission error instead of
//! trying to elevate behind the user's back.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// One line of the hosts file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostEntry {
    /// IP address as written.
    pub ip: String,
    /// Host names on the same line.
    pub names: Vec<String>,
    /// Trailing `#` comment, when present.
    pub comment: Option<String>,
    /// 1-based line in the file.
    pub line: usize,
}

/// Parsed hosts file plus the raw lines, so edits round-trip untouched
/// comments and ordering.
#[derive(Debug, Clone, Default)]
pub struct HostsFile {
    /// Parsed entries.
    pub entries: Vec<HostEntry>,
    /// Raw lines (without trailing newline).
    pub lines: Vec<String>,
}

/// Parse hosts file text.
pub fn parse(text: &str) -> HostsFile {
    let lines: Vec<String> = text
        .lines()
        .map(|l| l.trim_end_matches('\r').to_string())
        .collect();
    let mut entries = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        // Strip a trailing comment off the entry itself.
        let (body, comment) = match trimmed.split_once('#') {
            Some((body, comment)) => (body.trim(), Some(comment.trim().to_string())),
            None => (trimmed, None),
        };
        let mut parts = body.split_whitespace();
        let Some(ip) = parts.next() else {
            continue;
        };
        let names: Vec<String> = parts.map(str::to_string).collect();
        if names.is_empty() {
            continue;
        }
        entries.push(HostEntry {
            ip: ip.to_string(),
            names,
            comment,
            line: index + 1,
        });
    }
    HostsFile { entries, lines }
}

/// Read and parse the hosts file at `path`.
pub fn load(path: &Path) -> std::io::Result<HostsFile> {
    let text = std::fs::read_to_string(path)?;
    Ok(parse(&text))
}

/// Look up a host name (first match wins, like the resolver).
pub fn lookup<'a>(hosts: &'a HostsFile, name: &str) -> Option<&'a HostEntry> {
    hosts
        .entries
        .iter()
        .find(|entry| entry.names.iter().any(|n| n.eq_ignore_ascii_case(name)))
}

/// Add (or update) `names -> ip`.
///
/// If any of the names already maps elsewhere, that line is rewritten to the
/// new ip so duplicates never accumulate. Returns the new file content.
pub fn add(hosts: &HostsFile, ip: &str, names: &[String], comment: Option<&str>) -> String {
    let mut lines = hosts.lines.clone();
    let target: Vec<&str> = names.iter().map(String::as_str).collect();

    // Rewrite lines that already carry one of the names with a different ip.
    for entry in &hosts.entries {
        let shares_name = entry
            .names
            .iter()
            .any(|n| target.iter().any(|t| n.eq_ignore_ascii_case(t)));
        if shares_name && entry.ip != ip {
            if let Some(line) = lines.get_mut(entry.line - 1) {
                *line = format!("{ip} {}", entry.names.join(" "));
            }
        }
    }

    // Drop the name from the just-rewritten entries if it also appears
    // elsewhere, then append one fresh line when nothing carries the mapping.
    let refreshed = parse(&lines.join("\n"));
    let already =
        lookup(&refreshed, names.first().unwrap_or(&String::new())).is_some_and(|e| e.ip == ip);
    if !already {
        let mut new_line = format!("{ip} {}", names.join(" "));
        if let Some(comment) = comment {
            new_line.push_str(&format!("  # {comment}"));
        }
        if !lines.is_empty() && lines.last().map(|l| !l.trim().is_empty()) == Some(true) {
            lines.push(String::new());
        }
        lines.push(new_line);
    }
    lines.join("\n") + "\n"
}

/// Remove every entry whose first name or any alias matches `name`.
/// Returns `None` when no entry matched (caller reports not-found).
pub fn remove(hosts: &HostsFile, name: &str) -> Option<String> {
    let mut lines = hosts.lines.clone();
    // Collect line numbers first, then blank them from the back.
    let mut doomed: Vec<usize> = hosts
        .entries
        .iter()
        .filter(|entry| entry.names.iter().any(|n| n.eq_ignore_ascii_case(name)))
        .map(|entry| entry.line - 1)
        .collect();
    if doomed.is_empty() {
        return None;
    }
    doomed.sort_unstable();
    for index in doomed.into_iter().rev() {
        lines.remove(index);
    }
    Some(lines.join("\n") + "\n")
}

/// Render a diff-friendly summary: `ip  names` per line.
pub fn render(hosts: &HostsFile) -> Vec<String> {
    hosts
        .entries
        .iter()
        .map(|entry| format!("{}  {}", entry.ip, entry.names.join(" ")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "127.0.0.1 localhost\n# a comment\n10.0.0.5 web api # staging\n\n";

    #[test]
    fn parse_keeps_comments_and_line_numbers() {
        let hosts = parse(SAMPLE);
        assert_eq!(hosts.entries.len(), 2);
        assert_eq!(hosts.entries[1].names, vec!["web", "api"]);
        assert_eq!(hosts.entries[1].comment.as_deref(), Some("staging"));
        assert_eq!(hosts.entries[1].line, 3);
    }

    #[test]
    fn lookup_is_case_insensitive_and_first_match_wins() {
        let hosts = parse("127.0.0.1 a\n127.0.0.2 A\n");
        assert_eq!(lookup(&hosts, "A").unwrap().ip, "127.0.0.1");
    }

    #[test]
    fn add_updates_existing_mapping_instead_of_duplicating() {
        let hosts = parse("10.0.0.5 web\n");
        let updated = add(&hosts, "10.0.0.9", &["web".to_string()], None);
        let after = parse(&updated);
        assert_eq!(lookup(&after, "web").unwrap().ip, "10.0.0.9");
        assert_eq!(after.entries.len(), 1);
    }

    #[test]
    fn add_appends_when_absent_and_remove_deletes_every_alias_line() {
        let hosts = parse("127.0.0.1 localhost\n");
        let text = add(
            &hosts,
            "10.1.1.1",
            &["web".into(), "api".into()],
            Some("dev"),
        );
        let after = parse(&text);
        assert_eq!(lookup(&after, "api").unwrap().ip, "10.1.1.1");

        let removed = remove(&after, "web").expect("entry exists");
        assert!(parse(&removed).entries.len() == 1);
        assert!(remove(&after, "missing").is_none());
    }
}
