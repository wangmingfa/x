//! Schedule adapter: list / add / remove through each platform's own tool.
//!
//! Grammar per OS (documented at [`x_core::schedule::ScheduleManager::add`]):
//! - Linux: cron syntax (`*/5 * * * *`), written with `crontab -`; systemd
//!   timers appear in the listing read-only.
//! - Windows: `schtasks /create` flags — `daily`, `hourly`, `onlogon` or a
//!   `minute`/`minute=N` interval; listed via `/query /fo csv`.
//! - macOS: launchd user jobs with an `EveryNMinutes`-style interval, written
//!   into `~/Library/LaunchAgents/x-<name>.plist`; `crontab` entries also
//!   appear in the listing when `crontab` exists.

#[cfg(target_os = "macos")]
use std::path::PathBuf;
use std::process::Command;
use x_core::error::{Error, Result};
use x_core::schedule::{ScheduleEntry, ScheduleManager};

/// The platform schedule adapter.
pub struct PlatformSchedule;

impl ScheduleManager for PlatformSchedule {
    fn list(&self) -> Result<Vec<ScheduleEntry>> {
        #[cfg(windows)]
        {
            windows_list()
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            linux_list()
        }
        #[cfg(target_os = "macos")]
        {
            macos_list()
        }
        #[cfg(not(any(unix, windows)))]
        Err(Error::unsupported("schedule listing is not supported here"))
    }

    fn add(&self, name: &str, schedule: &str, command: &str) -> Result<()> {
        #[cfg(windows)]
        {
            // schtasks grammar: /sc minute|hourly|daily|onlogon with /mo.
            let (sc, mo) = parse_windows_schedule(schedule)?;
            let mut args = vec![
                "/create".to_string(),
                "/f".to_string(),
                "/tn".to_string(),
                name.to_string(),
                "/sc".to_string(),
                sc,
            ];
            if let Some(mo) = mo {
                args.push("/mo".to_string());
                args.push(mo);
            }
            args.push("/tr".to_string());
            args.push(command.to_string());
            run_schtasks(&args)
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            // crontab lines carry no name; the listing addresses them as
            // `crontab:<line>` and `remove` takes that back.
            let _ = name;
            let cron_line = format!("{schedule} {command}");
            append_crontab(&cron_line)?;
            Ok(())
        }
        #[cfg(target_os = "macos")]
        {
            let minutes = parse_interval_minutes(schedule)?;
            write_launchd_job(name, minutes, command)?;
            run(
                "launchctl",
                &["load", &launchd_path(name).to_string_lossy()],
            )
        }
        #[cfg(not(any(unix, windows)))]
        return Err(Error::unsupported("schedule add is not supported here"));
    }

    fn remove(&self, name: &str) -> Result<()> {
        #[cfg(windows)]
        {
            run_schtasks(&[
                "/delete".to_string(),
                "/f".to_string(),
                "/tn".to_string(),
                name.to_string(),
            ])
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            remove_crontab_line(name)?;
            Ok(())
        }
        #[cfg(target_os = "macos")]
        {
            let path = launchd_path(name);
            if !path.exists() {
                return Err(Error::not_found(format!("no scheduled job named {name}")));
            }
            let _ = Command::new("launchctl")
                .args(["unload", &path.to_string_lossy()])
                .output();
            std::fs::remove_file(&path)
                .map_err(|e| Error::system(format!("cannot delete {}: {e}", path.display())))?;
            Ok(())
        }
        #[cfg(not(any(unix, windows)))]
        return Err(Error::unsupported("schedule remove is not supported here"));
    }
}

#[cfg(target_os = "macos")]
fn run(program: &str, args: &[&str]) -> Result<()> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| Error::system(format!("cannot run {program}: {e}")))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let mut error = Error::system(format!(
        "{program} failed: {}",
        if stderr.is_empty() {
            "non-zero exit"
        } else {
            &stderr
        }
    ));
    // crontab / schtasks / launchctl all need elevation for machine-level
    // stores; the CLI shows the structured hint.
    error = error.with_permission(if cfg!(target_family = "windows") {
        x_core::PermissionRequirement::Administrator
    } else {
        x_core::PermissionRequirement::Root
    });
    Err(error)
}

#[cfg(windows)]
fn run_schtasks(args: &[String]) -> Result<()> {
    let output = Command::new("schtasks")
        .args(args)
        .output()
        .map_err(|e| Error::system(format!("cannot run schtasks: {e}")))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(Error::system(format!(
        "schtasks failed: {}",
        if stderr.is_empty() {
            "non-zero exit"
        } else {
            &stderr
        }
    )))
}

/// Accept `daily`, `hourly`, `onlogon`, `every=15` (minutes) or `minute=15`.
#[cfg(windows)]
fn parse_windows_schedule(schedule: &str) -> Result<(String, Option<String>)> {
    let lowered = schedule.trim().to_ascii_lowercase();
    match lowered.as_str() {
        "daily" => Ok(("daily".to_string(), None)),
        "hourly" => Ok(("hourly".to_string(), None)),
        "onlogon" | "on-startup" | "onstartup" => Ok(("onlogon".to_string(), None)),
        other => {
            let minutes = other
                .strip_prefix("every=")
                .or_else(|| other.strip_prefix("minute="))
                .and_then(|v| v.parse::<u32>().ok());
            match minutes {
                Some(m) if m > 0 => Ok(("minute".to_string(), Some(m.to_string()))),
                _ => Err(Error::invalid_input(format!(
                    "unsupported schedule `{schedule}`; use daily | hourly | onlogon | every=<minutes>"
                ))),
            }
        }
    }
}

#[cfg(windows)]
fn windows_list() -> Result<Vec<ScheduleEntry>> {
    let output = Command::new("schtasks")
        .args(["/query", "/fo", "csv", "/nh"])
        .output()
        .map_err(|e| Error::unsupported(format!("schtasks is not available: {e}")))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut rows = Vec::new();
    for record in csv_records(&text) {
        // CSV: TaskName, Next Run Time, Status
        if record.len() < 3 {
            continue;
        }
        let name = record[0].trim().to_string();
        if name.starts_with("\\Microsoft") {
            continue; // OS noise; user tasks first
        }
        let status = record[2].trim().to_string();
        rows.push(ScheduleEntry {
            name: name.trim_start_matches('\\').to_string(),
            command: String::new(),
            trigger: None,
            enabled: !status.contains("Disabled"),
        });
    }
    Ok(rows)
}

/// Minimal CSV reader good enough for schtasks /fo csv (quoted, no escapes).
#[cfg(windows)]
fn csv_records(text: &str) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    for line in text.lines() {
        let mut fields = Vec::new();
        let mut current = String::new();
        let mut in_quotes = false;
        for ch in line.chars() {
            match ch {
                '"' => in_quotes = !in_quotes,
                ',' if !in_quotes => {
                    fields.push(std::mem::take(&mut current));
                }
                _ => current.push(ch),
            }
        }
        fields.push(current);
        if fields.len() > 1 {
            rows.push(fields);
        }
    }
    rows
}

#[cfg(all(unix, not(target_os = "macos")))]
fn linux_list() -> Result<Vec<ScheduleEntry>> {
    let mut rows = Vec::new();
    if let Ok(output) = Command::new("crontab").arg("-l").output() {
        let text = String::from_utf8_lossy(&output.stdout);
        for (index, line) in text.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            let mut fields = trimmed.splitn(6, ' ');
            let spec = format!(
                "{} {} {} {} {}",
                fields.next().unwrap_or_default(),
                fields.next().unwrap_or_default(),
                fields.next().unwrap_or_default(),
                fields.next().unwrap_or_default(),
                fields.next().unwrap_or_default(),
            );
            let command = fields.next().unwrap_or_default().to_string();
            rows.push(ScheduleEntry {
                name: format!("crontab:{}", index + 1),
                command,
                trigger: Some(x_core::schedule::ScheduleTrigger { spec }),
                enabled: true,
            });
        }
    }
    Ok(rows)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn append_crontab(line: &str) -> Result<()> {
    let existing = Command::new("crontab")
        .arg("-l")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    let updated = format!("{existing}{}\n", line);
    run_crontab_stdin(&updated)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn remove_crontab_line(name: &str) -> Result<()> {
    let output = Command::new("crontab")
        .arg("-l")
        .output()
        .map_err(|e| Error::unsupported(format!("crontab is not available: {e}")))?;
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    // Lines are addressed as `crontab:<n>` in the listing.
    let Some(index) = name.strip_prefix("crontab:") else {
        return Err(Error::invalid_input(format!(
            "crontab entries are addressed as `crontab:<line>` (from `x schedule list`), got `{name}`"
        )));
    };
    let line_number: usize = index
        .parse()
        .map_err(|_| Error::invalid_input(format!("bad crontab line `{name}`")))?;
    let mut kept: Vec<&str> = Vec::new();
    let mut current_line = 0;
    let mut removed = false;
    for line in text.lines() {
        current_line += 1;
        if current_line == line_number {
            removed = true;
            continue;
        }
        kept.push(line);
    }
    if !removed {
        return Err(Error::not_found(format!("no crontab line {name}")));
    }
    run_crontab_stdin(&format!("{}\n", kept.join("\n")))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn run_crontab_stdin(content: &str) -> Result<()> {
    use std::io::Write;
    let mut child = Command::new("crontab")
        .arg("-")
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| Error::system(format!("cannot run crontab: {e}")))?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(content.as_bytes())
            .map_err(|e| Error::system(format!("cannot write crontab: {e}")))?;
    }
    drop(child.stdin.take());
    let status = child
        .wait()
        .map_err(|e| Error::system(format!("crontab failed: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::system("crontab rejected the new table"))
    }
}

#[cfg(target_os = "macos")]
fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/Users/unknown"))
}

#[cfg(target_os = "macos")]
fn launchd_path(name: &str) -> PathBuf {
    home_dir()
        .join("Library/LaunchAgents")
        .join(format!("x-{name}.plist"))
}

/// Accept `every=15` / `minute=15` / a bare number of minutes.
#[cfg(target_os = "macos")]
fn parse_interval_minutes(schedule: &str) -> Result<u64> {
    let lowered = schedule.trim().to_ascii_lowercase();
    let value = lowered
        .strip_prefix("every=")
        .or_else(|| lowered.strip_prefix("minute="))
        .unwrap_or(&lowered);
    value.parse::<u64>().ok().filter(|m| *m > 0).ok_or_else(|| {
        Error::invalid_input(format!(
            "unsupported schedule `{schedule}`; use a number of minutes or every=<minutes>"
        ))
    })
}

#[cfg(target_os = "macos")]
fn write_launchd_job(name: &str, minutes: u64, command: &str) -> Result<()> {
    let path = launchd_path(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| Error::system(format!("cannot create {}: {e}", parent.display())))?;
    }
    let program = shell_words_split(command);
    let mut plist = String::new();
    plist.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    plist.push_str("<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n");
    plist.push_str("<plist version=\"1.0\">\n<dict>\n");
    plist.push_str("  <key>Label</key>\n");
    plist.push_str(&format!("  <string>x-{name}</string>\n"));
    plist.push_str("  <key>ProgramArguments</key>\n  <array>\n");
    for word in &program {
        plist.push_str(&format!("    <string>{}</string>\n", escape_xml(word)));
    }
    plist.push_str("  </array>\n");
    plist.push_str("  <key>StartInterval</key>\n");
    plist.push_str(&format!("  <integer>{minutes}</integer>\n"));
    plist.push_str("</dict>\n</plist>\n");
    std::fs::write(&path, plist)
        .map_err(|e| Error::system(format!("cannot write {}: {e}", path.display())))
}

/// Split a command into words the way a shell would, for ProgramArguments.
/// Quotes are honoured; backslash escapes a quote. Good enough for the
/// one-line commands people actually schedule.
#[cfg(target_os = "macos")]
fn shell_words_split(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for ch in command.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' if quote != Some('\'') => escaped = true,
            '\'' | '"' if quote.is_none() => quote = Some(ch),
            c if Some(c) == quote => quote = None,
            c if quote.is_none() && c.is_whitespace() => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

#[cfg(target_os = "macos")]
fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(target_os = "macos")]
fn macos_list() -> Result<Vec<ScheduleEntry>> {
    let mut rows = Vec::new();
    let agents = home_dir().join("Library/LaunchAgents");
    if let Ok(entries) = std::fs::read_dir(&agents) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = match path.file_stem().and_then(|n| n.to_str()) {
                Some(n) => n.strip_prefix("x-").unwrap_or(n).to_string(),
                None => continue,
            };
            if path.extension().is_none_or(|e| e != "plist") {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let interval = text
                .lines()
                .find(|l| l.contains("<integer>"))
                .and_then(|l| {
                    l.split(['<', '>'])
                        .nth(2)
                        .and_then(|v| v.parse::<u64>().ok())
                })
                .map(|m| format!("every={m}"));
            rows.push(ScheduleEntry {
                name,
                command: text
                    .lines()
                    .find(|l| l.contains("<string>") && !l.contains("Label"))
                    .map(|l| l.split(['<', '>']).nth(2).unwrap_or_default().to_string())
                    .unwrap_or_default(),
                trigger: interval.map(|spec| x_core::schedule::ScheduleTrigger { spec }),
                enabled: true,
            });
        }
    }
    Ok(rows)
}
