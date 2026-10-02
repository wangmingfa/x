//! Containers, through whichever engine CLI the machine actually has.
//!
//! Same stance as `gitcmd`: the engine belongs to the engine, x renders its
//! output in x's vocabulary. What this module adds is **engine selection**
//! rather than a new vocabulary: `docker`, `podman` and `nerdctl` speak
//! different CLIs, so each gets one adapter and the same five questions are
//! asked of all of them.
//!
//! What is deliberately *not* here is a merged container type pretending the
//! engines agree. They do not: podman reports `podman ps` columns in a
//! different order and shape, and its rootless port publishing differs. Where a
//! fact is not available the field stays `None`, and where a whole question has
//! no equivalent the adapter reports `Unsupported` — the same honesty rule the
//! rest of x follows.
//!
//! The engine is chosen once per invocation from the first CLI that answers, so
//! a machine with both docker and podman installed behaves predictably and
//! `X_CONTAINER_ENGINE` overrides the choice.

use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;

/// Environment variable that pins the engine, bypassing discovery.
const ENGINE_VAR: &str = "X_CONTAINER_ENGINE";

/// Which container engine a call went to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Engine {
    /// The `docker` CLI. The default so a struct literal without an engine
    /// still names a real one rather than failing to build.
    #[default]
    Docker,
    /// The `podman` CLI, including its rootless mode.
    Podman,
    /// The `nerdctl` CLI, which fronts containerd.
    Nerdctl,
}

impl Engine {
    /// The CLI binary name.
    pub const fn cli(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Podman => "podman",
            Self::Nerdctl => "nerdctl",
        }
    }

    /// Parse an engine name as a user would type it.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "docker" => Some(Self::Docker),
            "podman" => Some(Self::Podman),
            "nerdctl" | "containerd" => Some(Self::Nerdctl),
            _ => None,
        }
    }

    /// Every engine, in discovery order.
    pub const ALL: [Self; 3] = [Self::Docker, Self::Podman, Self::Nerdctl];
}

impl std::fmt::Display for Engine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.cli())
    }
}

/// One running or stopped container.
///
/// The `engine` field is not decoration: with two engines on one machine the
/// same id can belong to different containers, so a row that omitted it could
/// not be acted on.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContainerInfo {
    /// Which engine reported it.
    pub engine: Engine,
    /// Container id, short as the engine prints it.
    pub id: String,
    /// Image it runs from.
    pub image: String,
    /// Given name.
    pub name: String,
    /// `Up 3 hours`, `Exited (0) 2 minutes ago`, ...
    pub state: String,
    /// Published ports string, verbatim from the engine.
    pub ports: String,
}

/// One image.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ImageInfo {
    /// Which engine reported it.
    pub engine: Engine,
    /// Repository (`nginx`, `ghcr.io/x/y`).
    pub repository: String,
    /// Tag (`latest`).
    pub tag: String,
    /// Short image id.
    pub id: String,
    /// Human size (`187MB`).
    pub size: String,
}

/// One published host port binding of a running container.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContainerPort {
    /// Which engine published it.
    pub engine: Engine,
    /// Container name.
    pub name: String,
    /// Host address the port is published on.
    pub host_ip: String,
    /// Host port.
    pub host_port: u16,
    /// Protocol (`tcp` / `udp`).
    pub protocol: String,
}

/// Recent log lines of one container, oldest first.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ContainerLogs {
    /// Which engine served them.
    pub engine: Engine,
    /// Container they came from.
    pub container: String,
    /// Lines, oldest first.
    pub lines: Vec<String>,
}

/// The engine this call will use, chosen by `X_CONTAINER_ENGINE` or discovery.
///
/// Discovery asks each engine whether its CLI is present *and* its backend
/// answers, so a docker CLI with a stopped daemon does not shadow a working
/// podman.
pub fn active_engine() -> Result<Engine> {
    if let Some(pinned) = std::env::var_os(ENGINE_VAR) {
        let raw = pinned.to_string_lossy().into_owned();
        return Engine::parse(&raw).ok_or_else(|| {
            Error::invalid_input(format!(
                "{ENGINE_VAR}={raw} is not a known engine; try docker, podman or nerdctl"
            ))
        });
    }
    Engine::ALL
        .into_iter()
        .find(|engine| cli_answers(*engine))
        .ok_or_else(|| {
            Error::unsupported(
                "no container engine found: install docker, podman or nerdctl, or set X_CONTAINER_ENGINE",
            )
        })
}

/// Every engine whose CLI is installed and whose backend answers.
pub fn available_engines() -> Vec<Engine> {
    Engine::ALL
        .into_iter()
        .filter(|engine| cli_answers(*engine))
        .collect()
}

/// Whether this engine's CLI is installed and its backend is reachable.
///
/// `info` is the cheapest call that proves both: a missing binary fails to
/// spawn, and a stopped daemon exits non-zero.
fn cli_answers(engine: Engine) -> bool {
    let probe: &[&str] = match engine {
        Engine::Docker => &["info", "--format", "{{.ServerVersion}}"],
        Engine::Podman => &["info", "--format", "{{.Version}}"],
        Engine::Nerdctl => &["info"],
    };
    Command::new(engine.cli())
        .args(probe)
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Containers across the active engine, `-a` including stopped ones.
pub fn containers(all: bool) -> Result<Vec<ContainerInfo>> {
    containers_with(active_engine()?, all)
}

/// Containers from one named engine.
///
/// `x docker` calls this with `Engine::Docker` so it never silently answers
/// from a podman that happens to also be installed; `x container` calls
/// [`containers`] and lets discovery decide.
pub fn containers_with(engine: Engine, all: bool) -> Result<Vec<ContainerInfo>> {
    let format = "{{.ID}}\t{{.Image}}\t{{.Names}}\t{{.Status}}\t{{.Ports}}";
    let mut args = vec!["ps", "--format", format];
    if all {
        args.push("-a");
    }
    let text = run(engine, &args)?;
    Ok(parse_ps(engine, &text))
}

/// Images across the active engine, newest first.
pub fn images() -> Result<Vec<ImageInfo>> {
    images_with(active_engine()?)
}

/// Images from one named engine.
pub fn images_with(engine: Engine) -> Result<Vec<ImageInfo>> {
    let text = run(
        engine,
        &[
            "images",
            "--format",
            "{{.Repository}}\t{{.Tag}}\t{{.ID}}\t{{.Size}}",
        ],
    )?;
    Ok(parse_images(engine, &text))
}

/// Every published host port across running containers of the active engine.
pub fn ports() -> Result<Vec<ContainerPort>> {
    ports_with(active_engine()?)
}

/// Every published host port of one named engine.
pub fn ports_with(engine: Engine) -> Result<Vec<ContainerPort>> {
    let text = run(
        engine,
        &["ps", "--format", "{{.ID}}\t{{.Names}}\t{{.Ports}}"],
    )?;
    Ok(parse_ports(engine, &text))
}

/// Which container publishes `host_port`, if any.
pub fn port_owner(host_port: u16) -> Result<Option<ContainerPort>> {
    Ok(ports()?.into_iter().find(|p| p.host_port == host_port))
}

/// Last `lines` log lines of one container, oldest first.
pub fn logs(container: &str, lines: usize) -> Result<ContainerLogs> {
    logs_with(active_engine()?, container, lines)
}

/// Last `lines` log lines of one container on one named engine.
pub fn logs_with(engine: Engine, container: &str, lines: usize) -> Result<ContainerLogs> {
    let count = lines.to_string();
    let text = run(engine, &["logs", "--tail", &count, container])?;
    Ok(ContainerLogs {
        engine,
        container: container.to_string(),
        lines: text.lines().map(str::to_string).collect(),
    })
}

/// Run the engine's CLI and return stdout, mapping failures onto x's errors.
fn run(engine: Engine, args: &[&str]) -> Result<String> {
    let output = Command::new(engine.cli())
        .args(args)
        .output()
        .map_err(|e| Error::unsupported(format!("{engine} is not available: {e}")))?;
    if !output.status.success() {
        return Err(engine_error(
            engine,
            &String::from_utf8_lossy(&output.stderr),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Map a failed engine call onto an x error.
///
/// The wording differs per engine — docker says "Cannot connect to the Docker
/// daemon", podman and nerdctl say their own — so the check is on the shape of
/// the complaint, not on one string.
fn engine_error(engine: Engine, stderr: &str) -> Error {
    let message = stderr.trim();
    let lower = message.to_ascii_lowercase();
    let backend_down = lower.contains("cannot connect")
        || lower.contains("daemon is not running")
        || lower.contains("is the docker daemon running")
        || lower.contains("connection refused")
        || lower.contains("cannot connect to the daemon")
        || lower.contains("is the podman daemon running")
        // nerdctl words it as a containerd connection failure.
        || lower.contains("failed to connect to containerd");
    if backend_down {
        Error::unsupported(format!("{engine} backend is not running"))
    } else {
        Error::system(format!("{engine} failed: {message}"))
    }
}

/// Parse `ps` output: id, image, name, state, ports, tab separated.
fn parse_ps(engine: Engine, text: &str) -> Vec<ContainerInfo> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut fields = line.split('\t');
            ContainerInfo {
                engine,
                id: pop(&mut fields),
                image: pop(&mut fields),
                name: pop(&mut fields),
                state: pop(&mut fields),
                ports: pop(&mut fields),
            }
        })
        .collect()
}

/// Parse `images` output: repository, tag, id, size, tab separated.
fn parse_images(engine: Engine, text: &str) -> Vec<ImageInfo> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let mut fields = line.split('\t');
            ImageInfo {
                engine,
                repository: pop(&mut fields),
                tag: pop(&mut fields),
                id: pop(&mut fields),
                size: pop(&mut fields),
            }
        })
        .collect()
}

/// Parse the published-port column across every running container.
///
/// The engine's own format is kept verbatim in the string field; only the
/// host/port pairs are split out, because `0.0.0.0:8080->80/tcp` and
/// `:::8080->80/tcp` both have to be understood and neither should be
/// re-worded on the way through.
fn parse_ports(engine: Engine, text: &str) -> Vec<ContainerPort> {
    let mut rows = Vec::new();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let mut fields = line.split('\t');
        let _id = pop(&mut fields);
        let name = pop(&mut fields);
        let mapping = pop(&mut fields);
        for part in mapping.split(',') {
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
                    engine,
                    name: name.clone(),
                    // `:::` and `0.0.0.0` both mean every interface; keeping the
                    // engine's own spelling avoids inventing a fourth form.
                    host_ip: ip.to_string(),
                    host_port,
                    protocol: protocol.to_string(),
                });
            }
        }
    }
    rows
}

/// Whether any container engine is usable right now.
pub fn available(_dir: &Path) -> bool {
    active_engine().is_ok()
}

/// Next whitespace-delimited field, trimmed and defaulted.
fn pop(fields: &mut std::str::Split<char>) -> String {
    fields.next().unwrap_or_default().trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;

    #[test]
    fn engine_names_round_trip() {
        for engine in Engine::ALL {
            assert_eq!(Engine::parse(engine.cli()), Some(engine));
            assert_eq!(engine.to_string(), engine.cli());
        }
        assert_eq!(Engine::parse("containerd"), Some(Engine::Nerdctl), "alias");
        assert_eq!(Engine::parse(" DOCKER "), Some(Engine::Docker), "forgiving");
        assert_eq!(Engine::parse("lima"), None, "unknown stays unknown");
    }

    #[test]
    fn podman_output_parses_like_dockers() {
        // podman's `--format` honours the same template fields; what differs is
        // the column *content*, not the shape, so one parser serves both. The
        // separators are real tabs, exactly as the template's `\t` emits.
        let text = "9f1c2b3a4d5e\tnginx:latest\tweb\tUp 3 hours\t0.0.0.0:8080->80/tcp\n";
        let rows = parse_ps(Engine::Podman, text);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].engine, Engine::Podman);
        assert_eq!(rows[0].id, "9f1c2b3a4d5e");
        assert_eq!(rows[0].image, "nginx:latest");
        assert_eq!(rows[0].name, "web");
        assert_eq!(rows[0].state, "Up 3 hours");
        assert_eq!(rows[0].ports, "0.0.0.0:8080->80/tcp");
    }

    #[test]
    fn nerdctl_rows_keep_their_own_shape() {
        // nerdctl prints a shorter status and nothing at all where docker
        // prints an em dash; both must survive as the engine's own text.
        let text = "a1b2c3\talpine:3.19\t—\t—\t\n";
        let rows = parse_ps(Engine::Nerdctl, text);
        assert_eq!(rows[0].engine, Engine::Nerdctl);
        assert_eq!(rows[0].name, "—");
        assert_eq!(rows[0].state, "—");
        assert_eq!(rows[0].ports, "");
    }

    #[test]
    fn images_carry_their_engine() {
        let text = "nginx\tlatest\t9f1c2b3a4d5e\t187MB\n";
        let rows = parse_images(Engine::Docker, text);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].repository, "nginx");
        assert_eq!(rows[0].tag, "latest");
        assert_eq!(rows[0].size, "187MB");
        assert_eq!(rows[0].engine, Engine::Docker);
    }

    #[test]
    fn published_ports_split_without_rewording_them() {
        let text = "abc\tweb\t0.0.0.0:8080->80/tcp, :::8443->443/tcp\n";
        let rows = parse_ports(Engine::Docker, text);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].host_ip, "0.0.0.0");
        assert_eq!(rows[0].host_port, 8080);
        assert_eq!(rows[0].protocol, "tcp");
        // The wildcard form is the engine's own spelling, kept verbatim.
        assert_eq!(rows[1].host_ip, "::");
        assert_eq!(rows[1].host_port, 8443);
    }

    #[test]
    fn a_row_with_no_published_ports_yields_nothing() {
        let rows = parse_ports(Engine::Podman, "abc\tdb\t\n");
        assert!(rows.is_empty(), "{rows:?}");
    }

    #[test]
    fn a_stopped_backend_is_unsupported_not_a_system_error() {
        // Each engine words this differently; the classification must not.
        for stderr in [
            "Cannot connect to the Docker daemon at unix:///var/run/docker.sock.",
            "Cannot connect to Podman socket. Is the podman daemon running?",
            "error: failed to connect to containerd",
            "connection refused",
        ] {
            let err = engine_error(Engine::Docker, stderr);
            assert_eq!(err.kind(), ErrorKind::Unsupported, "{stderr}");
        }
    }

    #[test]
    fn a_real_failure_stays_a_system_error() {
        let err = engine_error(Engine::Docker, "no such container: ghost");
        assert_eq!(err.kind(), ErrorKind::System, "{err}");
        assert!(err.to_string().contains("no such container"), "{err}");
    }

    #[test]
    fn a_blank_line_is_not_a_container() {
        let rows = parse_ps(Engine::Docker, "\n\nabc\timg\tname\tUp\t\n\n");
        assert_eq!(rows.len(), 1, "{rows:?}");
    }

    #[test]
    fn a_short_row_fills_the_missing_fields_rather_than_panicking() {
        // `ps` mid-write or an older engine can print fewer columns; a missing
        // field is empty, not a crash.
        let rows = parse_ps(Engine::Docker, "abc\timg\n");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "abc");
        assert_eq!(rows[0].name, "");
    }
}
