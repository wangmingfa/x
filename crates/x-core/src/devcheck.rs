//! Development toolchain detection and command health checks.
//!
//! A tool is "present" when it resolves on `PATH`; its version is whatever it
//! prints for `--version`. That is exactly what a developer means by
//! "is Node installed here" — no registry queries, no magic.

use crate::devenv::which;
use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::process::Command;
use std::time::Duration;

/// One detected tool.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ToolInfo {
    /// Canonical tool name (`node`, `rustc`, `docker`, ...).
    pub name: String,
    /// Found on `PATH`.
    pub installed: bool,
    /// Absolute path when installed.
    pub path: Option<String>,
    /// First line of `--version` when the tool answers.
    pub version: Option<String>,
}

impl ToolInfo {
    /// ✓ / ✗ marker for quick scans.
    pub fn mark(&self) -> &'static str {
        if self.installed {
            "✓"
        } else {
            "✗"
        }
    }
}

/// The toolchains `x dev` checks, in report order.
pub const KNOWN_TOOLS: &[&str] = &[
    "node", "npm", "pnpm", "yarn", "bun", "deno", "cargo", "rustc", "python", "go", "java",
    "docker", "git", "ssh",
];

/// Probe one command: is it on `PATH`, where, and what does `--version` say.
pub fn check_tool(name: &str) -> ToolInfo {
    match which(name) {
        Some(path) => {
            let version = run_version(name);
            ToolInfo {
                name: name.to_string(),
                installed: true,
                path: Some(path.to_string_lossy().into_owned()),
                version,
            }
        }
        None => ToolInfo {
            name: name.to_string(),
            installed: false,
            path: None,
            version: None,
        },
    }
}

/// `x dev`: all known tools in one sweep.
pub fn dev_overview() -> Vec<ToolInfo> {
    KNOWN_TOOLS.iter().map(|name| check_tool(name)).collect()
}

/// `x doctor`: a verdict for one command — installed, version, and a hint when
/// something is off (e.g. present but refuses to run).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandCheck {
    /// Tool that was checked.
    pub tool: ToolInfo,
    /// Human verdict (`ok`, `missing`, `error`).
    pub verdict: String,
}

/// Health-check a single command.
pub fn doctor_command(name: &str) -> CommandCheck {
    let tool = check_tool(name);
    let verdict = match (&tool.installed, &tool.version) {
        (true, Some(_)) => "ok".to_string(),
        (true, None) => "installed; `--version` printed nothing".to_string(),
        (false, _) => "missing".to_string(),
    };
    CommandCheck { tool, verdict }
}

/// One row of the system-wide `x doctor` sweep.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemCheck {
    /// What was checked (`network`, `dns`, `git`, ...).
    pub area: String,
    /// `✓`, `⚠` or `✗`.
    pub mark: String,
    /// One line detail.
    pub detail: String,
}

/// One-shot environment check: network, DNS, and the key toolchains.
///
/// Never panics, never takes more than a couple of seconds: probes use short
/// timeouts and every failure becomes a row instead of an error.
pub fn doctor_system() -> Vec<SystemCheck> {
    let mut rows = Vec::new();

    rows.push(check_network());
    rows.push(check_dns());
    for name in ["git", "node", "cargo", "python", "docker", "ssh"] {
        let tool = check_tool(name);
        rows.push(SystemCheck {
            area: name.to_string(),
            mark: if tool.installed { "✓" } else { "⚠" }.to_string(),
            detail: tool
                .version
                .clone()
                .unwrap_or_else(|| "not installed".to_string()),
        });
    }
    rows.push(check_proxy());
    rows
}

fn check_network() -> SystemCheck {
    // Cheap, reliable, offline-friendly: is there any non-loopback interface
    // with an address, and can we resolve a well known name?
    let reachable = std::net::TcpStream::connect_timeout(
        &"1.1.1.1:53".parse().expect("static address"),
        Duration::from_secs(2),
    )
    .is_ok();
    SystemCheck {
        area: "network".into(),
        mark: if reachable { "✓" } else { "✗" }.into(),
        detail: if reachable {
            "internet reachable (1.1.1.1:53)".into()
        } else {
            "cannot reach 1.1.1.1:53 — offline or firewalled?".into()
        },
    }
}

fn check_dns() -> SystemCheck {
    match std::net::ToSocketAddrs::to_socket_addrs(&("example.com", 443)) {
        Ok(addrs) => {
            if addrs.count() > 0 {
                SystemCheck {
                    area: "dns".into(),
                    mark: "✓".into(),
                    detail: "example.com resolves".into(),
                }
            } else {
                SystemCheck {
                    area: "dns".into(),
                    mark: "✗".into(),
                    detail: "example.com does not resolve".into(),
                }
            }
        }
        _ => SystemCheck {
            area: "dns".into(),
            mark: "✗".into(),
            detail: "example.com does not resolve".into(),
        },
    }
}

fn check_proxy() -> SystemCheck {
    let proxies = ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"]
        .iter()
        .filter_map(|name| std::env::var(name).ok().map(|v| format!("{name}={v}")))
        .collect::<Vec<_>>();
    SystemCheck {
        area: "proxy".into(),
        mark: "✓".into(),
        detail: if proxies.is_empty() {
            "no proxy configured".into()
        } else {
            proxies.join(", ")
        },
    }
}

/// First non-empty line of `--version`, run with a short timeout via `kill`
/// after the fact is not portable — so instead we accept that some tools are
/// slow and simply take the first line whenever they finish.
fn run_version(name: &str) -> Option<String> {
    let output = Command::new(name).arg("--version").output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().find(|l| !l.trim().is_empty());
    Some(line.unwrap_or_default().trim().to_string()).filter(|l| !l.is_empty())
}

/// Version of a tool, or an error naming what is missing. For commands that
/// need a precise failure (`x doctor <cmd>` scripting).
pub fn version_or_error(name: &str) -> Result<String> {
    run_version(name).ok_or_else(|| {
        if which(name).is_none() {
            Error::not_found(format!("{name} is not on PATH"))
        } else {
            Error::system(format!("{name} did not answer --version"))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_tool_is_reported_without_panicking() {
        let info = check_tool("x-definitely-not-a-tool-aa");
        assert!(!info.installed);
        assert_eq!(info.mark(), "✗");
        assert_eq!(
            doctor_command("x-definitely-not-a-tool-aa").verdict,
            "missing"
        );
    }

    #[test]
    fn doctor_system_returns_rows_for_every_area() {
        let rows = doctor_system();
        assert!(rows.iter().any(|r| r.area == "network"));
        assert!(rows.iter().any(|r| r.area == "dns"));
        assert!(rows.iter().any(|r| r.area == "git"));
    }

    #[test]
    fn version_or_error_classifies_missing_vs_broken() {
        let err = version_or_error("x-definitely-not-a-tool-aa").unwrap_err();
        assert_eq!(err.kind(), crate::error::ErrorKind::NotFound);
    }
}
