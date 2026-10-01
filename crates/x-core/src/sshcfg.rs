//! SSH hosts, straight from `~/.ssh/config`.
//!
//! The config file *is* the user's source of truth, so x parses it rather than
//! keeping its own database. `connect` / `ping` shell out to the `ssh` binary
//! the user has configured; if that binary is missing the error says so.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

/// One `Host` block from the SSH config.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SshHost {
    /// Pattern or alias (`Host *` produces `*`).
    pub pattern: String,
    /// First alias if several are on the `Host` line, else the pattern.
    pub name: String,
    /// `HostName` when set.
    pub hostname: Option<String>,
    /// `User` when set.
    pub user: Option<String>,
    /// `Port` when set.
    pub port: Option<u16>,
    /// `IdentityFile` entries, `~` expanded.
    pub identity_files: Vec<String>,
    /// Any other keywords, lower cased, verbatim values.
    pub extra: Vec<(String, String)>,
}

impl SshHost {
    /// `user@host:port` the way `ssh` would resolve it, for display.
    pub fn target(&self) -> String {
        let host = self
            .hostname
            .clone()
            .unwrap_or_else(|| self.pattern.clone());
        let host = expand_home(&host);
        match (&self.user, self.port) {
            (Some(user), Some(port)) => format!("{user}@{host}:{port}"),
            (Some(user), None) => format!("{user}@{host}"),
            (None, Some(port)) => format!("{host}:{port}"),
            (None, None) => host,
        }
    }
}

/// All concrete (non wildcard) hosts defined in the config file.
pub fn hosts_from_file(path: &Path) -> Result<Vec<SshHost>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| Error::not_found(format!("cannot read {}: {e}", path.display())))?;
    Ok(parse_config(&text))
}

/// Hosts from the default config, plus the count of known_hosts entries.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SshOverview {
    /// Concrete hosts in `~/.ssh/config`, in file order.
    pub hosts: Vec<SshHost>,
    /// Entries in `~/.ssh/known_hosts` (hashed entries count as one each).
    pub known_hosts: usize,
    /// Private key files sitting in `~/.ssh`.
    pub keys: Vec<String>,
}

/// Read everything x can see in `~/.ssh`.
pub fn overview(home: &Path) -> SshOverview {
    let ssh_dir = home.join(".ssh");
    let hosts = hosts_from_file(&ssh_dir.join("config"))
        .ok()
        .map(|all| {
            all.into_iter()
                .filter(|h| !h.pattern.contains('*'))
                .collect()
        })
        .unwrap_or_default();
    let known_hosts = std::fs::read_to_string(ssh_dir.join("known_hosts"))
        .map(|text| text.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0);
    let keys = list_keys(&ssh_dir);
    SshOverview {
        hosts,
        known_hosts,
        keys,
    }
}

/// `ssh -G` style sanity check: does the config resolve this alias.
///
/// Runs `ssh -G <alias>` and reports the resolved `hostname` / `user` /
/// `port` without opening a connection.
pub fn test(alias: &str) -> Result<SshHost> {
    let output = Command::new("ssh")
        .args(["-G", alias])
        .output()
        .map_err(|e| Error::unsupported(format!("ssh is not available: {e}")))?;
    if !output.status.success() {
        return Err(Error::not_found(format!(
            "ssh cannot resolve host: {alias}"
        )));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut host = SshHost {
        pattern: alias.to_string(),
        name: alias.to_string(),
        ..SshHost::default()
    };
    for line in text.lines() {
        let mut kv = line.splitn(2, ' ');
        let key = kv.next().unwrap_or_default();
        let value = kv.next().unwrap_or_default().trim();
        match key {
            "hostname" if host.hostname.is_none() => host.hostname = Some(value.to_string()),
            "user" if host.user.is_none() => host.user = Some(value.to_string()),
            "port" if host.port.is_none() => host.port = value.parse().ok(),
            "identityfile" => host.identity_files.push(expand_home(value)),
            _ => {}
        }
    }
    Ok(host)
}

/// TCP reachability probe: try to connect to `host:port` with a timeout.
///
/// `host` must already be a hostname or IP — this never resolves through ssh.
pub fn ping(host: &str, port: u16, timeout: std::time::Duration) -> Result<std::time::Duration> {
    use std::net::{TcpStream, ToSocketAddrs};
    use std::time::Instant;

    let started = Instant::now();
    let addresses = (host, port)
        .to_socket_addrs()
        .map_err(|e| Error::not_found(format!("cannot resolve {host}: {e}")))?;
    let mut last_error = None;
    for address in addresses {
        match TcpStream::connect_timeout(&address, timeout) {
            Ok(_) => return Ok(started.elapsed()),
            Err(e) => last_error = Some(e),
        }
    }
    Err(Error::system(format!(
        "cannot reach {host}:{port}: {}",
        last_error
            .map(|e| e.to_string())
            .unwrap_or_else(|| "no addresses".into())
    )))
}

/// Start an interactive `ssh` session. Blocks until the user logs out.
pub fn connect(target: &str) -> Result<()> {
    let status = Command::new("ssh")
        .arg(target)
        .status()
        .map_err(|e| Error::unsupported(format!("ssh is not available: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::system(format!(
            "ssh exited with {}",
            status.code().unwrap_or(-1)
        )))
    }
}

fn parse_config(text: &str) -> Vec<SshHost> {
    let mut hosts: Vec<SshHost> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.splitn(2, ['=', ' ']);
        let key = parts.next().unwrap_or_default().to_ascii_lowercase();
        let value = parts.next().unwrap_or_default().trim().to_string();
        if key != "host" {
            if let Some(current) = hosts.last_mut() {
                match key.as_str() {
                    "hostname" => current.hostname = Some(expand_home(&value)),
                    "user" => current.user = Some(value),
                    "port" => current.port = value.parse().ok(),
                    "identityfile" => current.identity_files.push(expand_home(&value)),
                    _ => current.extra.push((key, value)),
                }
            }
            continue;
        }
        let first = value.split_whitespace().next().unwrap_or(&value);
        hosts.push(SshHost {
            pattern: value.clone(),
            name: first.to_string(),
            ..SshHost::default()
        });
    }
    hosts
}

fn list_keys(ssh_dir: &Path) -> Vec<String> {
    const KEY_MARKERS: [&str; 4] = [
        "RSA PRIVATE KEY",
        "OPENSSH PRIVATE KEY",
        "EC PRIVATE KEY",
        "PRIVATE KEY",
    ];
    let Ok(entries) = std::fs::read_dir(ssh_dir) else {
        return Vec::new();
    };
    let mut keys = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".pub") || name == "known_hosts" || name == "config" {
            continue;
        }
        if let Ok(head) = std::fs::read_to_string(entry.path()) {
            let head: String = head.chars().take(200).collect();
            if KEY_MARKERS.iter().any(|marker| head.contains(marker)) {
                keys.push(name);
            }
        }
    }
    keys.sort();
    keys
}

fn expand_home(value: &str) -> String {
    if let Some(rest) = value.strip_prefix('~') {
        if let Ok(home) = std::env::var(if std::env::var("HOME").is_ok() {
            "HOME"
        } else {
            "USERPROFILE"
        }) {
            return format!("{home}{rest}");
        }
    }
    value.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_parser_reads_host_blocks() {
        let config = "\
# comment
Host web
  HostName example.com
  User deploy
  Port 2222
  IdentityFile ~/.ssh/id_ed25519

Host * !web
  ServerAliveInterval 60
";
        let hosts = parse_config(config);
        assert_eq!(hosts.len(), 2);
        let web = &hosts[0];
        assert_eq!(web.name, "web");
        assert_eq!(web.hostname.as_deref(), Some("example.com"));
        assert_eq!(web.user.as_deref(), Some("deploy"));
        assert_eq!(web.port, Some(2222));
        assert_eq!(web.target(), "deploy@example.com:2222");
        assert_eq!(
            hosts[1].extra,
            vec![("serveraliveinterval".into(), "60".into())]
        );
    }

    #[test]
    fn ping_reports_unreachable_clearly() {
        // Port 1 on localhost is almost surely closed; the error must mention the target.
        let err = ping("127.0.0.1", 1, std::time::Duration::from_millis(200)).unwrap_err();
        assert!(err.message().contains("127.0.0.1:1"));
    }
}
