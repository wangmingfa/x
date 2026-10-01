//! Docker through the `docker` CLI.
//!
//! Same stance as `gitcmd`: the daemon belongs to Docker, x just renders its
//! output in x's vocabulary. When the CLI or the daemon is missing, every
//! function returns `Unsupported` and `x capability` reports it.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

/// One running or stopped container.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContainerInfo {
    /// Container id (short).
    pub id: String,
    /// Image it runs from.
    pub image: String,
    /// Given name.
    pub name: String,
    /// `Up 3 hours`, `Exited (0) 2 minutes ago`, ...
    pub state: String,
    /// Published ports string as `docker ps` formats it.
    pub ports: String,
}

/// One image.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ImageInfo {
    /// Repository (`nginx`, `ghcr.io/x/y`).
    pub repository: String,
    /// Tag (`latest`).
    pub tag: String,
    /// Short image id.
    pub id: String,
    /// Human size (`187MB`).
    pub size: String,
}

/// One published port binding of a running container.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContainerPort {
    /// Container id (short).
    pub container: String,
    /// Container name.
    pub name: String,
    /// Host address the port is published on (`0.0.0.0`, `127.0.0.1`, ...).
    pub host_ip: String,
    /// Host port.
    pub host_port: u16,
    /// Protocol (`tcp` / `udp`).
    pub protocol: String,
}

/// Recent log lines of one container.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContainerLogs {
    /// Container they came from.
    pub container: String,
    /// Lines, oldest first.
    pub lines: Vec<String>,
}

/// Every container (`docker ps -a` when `all`).
pub fn containers(all: bool) -> Result<Vec<ContainerInfo>> {
    let mut args = vec![
        "ps",
        "--format",
        "{{.ID}}\t{{.Image}}\t{{.Names}}\t{{.Status}}\t{{.Ports}}",
    ];
    if all {
        args.push("-a");
    }
    let text = docker(&args)?;
    Ok(text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut fields = line.split('\t');
            ContainerInfo {
                id: pop(&mut fields),
                image: pop(&mut fields),
                name: pop(&mut fields),
                state: pop(&mut fields),
                ports: pop(&mut fields),
            }
        })
        .collect())
}

/// Images, newest first (`docker images`).
pub fn images() -> Result<Vec<ImageInfo>> {
    let text = docker(&[
        "images",
        "--format",
        "{{.Repository}}\t{{.Tag}}\t{{.ID}}\t{{.Size}}",
    ])?;
    Ok(text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut fields = line.split('\t');
            ImageInfo {
                repository: pop(&mut fields),
                tag: pop(&mut fields),
                id: pop(&mut fields),
                size: pop(&mut fields),
            }
        })
        .collect())
}

/// Every published host port across running containers.
pub fn ports() -> Result<Vec<ContainerPort>> {
    let text = docker(&["ps", "--format", "{{.ID}}\t{{.Names}}\t{{.Ports}}"])?;
    let mut rows = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let mut fields = line.split('\t');
        let id = pop(&mut fields);
        let name = pop(&mut fields);
        let mapping = pop(&mut fields);
        for part in mapping.split(',') {
            // `0.0.0.0:8080->80/tcp` or `127.0.0.1:5432->5432/tcp`
            let Some((host, container_side)) = part.trim().split_once("->") else {
                continue;
            };
            let Some((ip, port)) = host.rsplit_once(':') else {
                continue;
            };
            let (_cport, protocol) = container_side
                .split_once('/')
                .unwrap_or((container_side, "tcp"));
            if let Ok(host_port) = port.parse() {
                rows.push(ContainerPort {
                    container: id.clone(),
                    name: name.clone(),
                    host_ip: ip.to_string(),
                    host_port,
                    protocol: protocol.to_string(),
                });
            }
        }
    }
    Ok(rows)
}

/// Which container publishes `host_port`, if any.
pub fn port_owner(host_port: u16) -> Result<Option<ContainerPort>> {
    Ok(ports()?.into_iter().find(|p| p.host_port == host_port))
}

/// Last `lines` log lines of one container, oldest first.
pub fn logs(container: &str, lines: usize) -> Result<ContainerLogs> {
    let text = docker(&["logs", "--tail", &lines.to_string(), container])?;
    Ok(ContainerLogs {
        container: container.to_string(),
        lines: text.lines().map(|l| l.to_string()).collect(),
    })
}

fn pop(fields: &mut std::str::Split<char>) -> String {
    fields.next().unwrap_or_default().trim().to_string()
}

fn docker(args: &[&str]) -> Result<String> {
    let output = Command::new("docker")
        .args(args)
        .output()
        .map_err(|e| Error::unsupported(format!("docker is not available: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message = stderr.trim();
        let daemon_down = message.contains("Cannot connect to the Docker daemon");
        return Err(if daemon_down {
            Error::unsupported("docker daemon is not running")
        } else {
            Error::system(format!("docker failed: {message}"))
        });
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Whether `docker info` works right now (CLI present *and* daemon reachable).
pub fn available(_dir: &Path) -> bool {
    Command::new("docker")
        .args(["info", "--format", "{{.ServerVersion}}"])
        .output()
        .is_ok_and(|o| o.status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_daemon_is_an_unsupported_error() {
        // This asserts the *error classification*, which holds both when the
        // CLI is absent and when the daemon is down — the two states CI has.
        match containers(false) {
            Ok(_) => {} // docker running in this environment
            Err(e) => assert_eq!(e.kind(), crate::error::ErrorKind::Unsupported),
        }
    }
}
