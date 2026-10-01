//! Capability probing: what *this* host can actually do, per domain.
//!
//! The point is honesty at runtime. Roadmap tables say what `x` implements;
//! this module answers what the current machine answers with, by running the
//! cheap read operations and classifying what comes back:
//! `Supported` (works here), `Degraded` (works with reduced fidelity, or was
//! refused for a reason we can show) and `Unsupported` (the platform or the
//! detected manager has nothing behind it).
//!
//! Probes never mutate state: destructive features (kill, lifecycle actions)
//! are reported as implemented with a note that they were not exercised, and
//! the service probes use deliberately invalid names so an adapter can answer
//! "would this work" without doing anything.

use serde::{Deserialize, Serialize};

use crate::context::SystemContext;
use crate::error::{ErrorKind, Result};
use crate::network::PingRequest;
use crate::service::ServiceManagerType;
use std::net::{IpAddr, Ipv4Addr};

/// How well the current host supports one feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityStatus {
    /// Works on this host.
    Supported,
    /// Works with reduced fidelity, or needs privileges to work fully.
    Degraded,
    /// The platform or detected manager has no backing for this.
    Unsupported,
}

impl CapabilityStatus {
    /// Lowercase identifier, matching the JSON spelling.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Supported => "supported",
            Self::Degraded => "degraded",
            Self::Unsupported => "unsupported",
        }
    }
}

/// One probed feature.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Capability {
    /// Grouping key, e.g. `net` or `service`.
    pub domain: String,
    /// Human-readable feature name.
    pub feature: String,
    /// What the probe found.
    pub status: CapabilityStatus,
    /// Why it is degraded or unsupported, or extra context.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Run every probe against `context`.
pub fn probe(context: &SystemContext) -> Vec<Capability> {
    let mut out: Vec<Capability> = Vec::new();
    let mut push = |domain: &str, feature: &str, status, note: Option<String>| {
        out.push(Capability {
            domain: domain.to_string(),
            feature: feature.to_string(),
            status,
            note,
        });
    };

    // ---- system -----------------------------------------------------------
    let info = context.system.info();
    match &info {
        Ok(system) => push(
            "system",
            "system info",
            CapabilityStatus::Supported,
            Some(format!("{} {}", system.os_name, system.os_version)),
        ),
        Err(error) => {
            let (status, note) = classify_error(error);
            push("system", "system info", status, note);
        }
    }

    let cpu = context.system.cpu_usage();
    let (status, note) = match &cpu {
        Ok(_) => (CapabilityStatus::Supported, None),
        Err(error) => classify_error(error),
    };
    push("system", "cpu & memory usage", status, note);
    if let Ok(memory) = context.system.memory_usage() {
        let (status, note) = presence(
            memory.pressure,
            "no pressure interface exposed by this platform",
        );
        push("system", "memory pressure", status, note.map(String::from));
    }

    if let Ok(cpu) = &cpu {
        let (status, note) = presence(
            cpu.temperature_celsius,
            "no sensor is exposed on this platform",
        );
        push("system", "cpu temperature", status, note.map(String::from));
        let (status, note) = presence(
            cpu.governor.as_ref(),
            "scaling governors are a Linux concept",
        );
        push("system", "cpu governor", status, note.map(String::from));
        let (status, note) = if cpu.per_core_frequency_mhz.is_empty() {
            (
                CapabilityStatus::Degraded,
                Some("per-core frequencies are not exposed here".to_string()),
            )
        } else {
            (CapabilityStatus::Supported, None)
        };
        push("system", "per-core frequencies", status, note);
    }
    if let Ok(info) = &info {
        let (status, note) = presence(
            info.performance_cores,
            "the platform does not distinguish core classes",
        );
        push("system", "P/E core split", status, note.map(String::from));
    }

    // ---- process ----------------------------------------------------------
    let (status, note) = classify_call(&context.process.list(&Default::default()));
    push("process", "process list", status, note);
    let (status, note) = classify_call(&context.process.tree(&Default::default()));
    push("process", "process tree", status, note);

    let detail: Vec<&str> = match context.process.get(std::process::id()) {
        Ok(row) => {
            let mut missing = Vec::new();
            if row.cwd.is_none() {
                missing.push("cwd");
            }
            if row.open_files.is_none() {
                missing.push("open files");
            }
            if row.connections.is_none() {
                missing.push("connections");
            }
            if row.environment.is_none() {
                missing.push("environment");
            }
            if missing.is_empty() {
                Vec::new()
            } else {
                missing
            }
        }
        Err(_) => vec!["self lookup failed"],
    };
    push(
        "process",
        "process detail (cwd/files/env)",
        if detail.is_empty() {
            CapabilityStatus::Supported
        } else {
            CapabilityStatus::Degraded
        },
        (!detail.is_empty()).then(|| format!("missing: {}", detail.join(", "))),
    );
    push(
        "process",
        "kill process",
        CapabilityStatus::Supported,
        Some("implemented; not exercised by this probe, privileges apply".to_string()),
    );

    // ---- port --------------------------------------------------------------
    let (status, note) = classify_call(&context.port.list(&Default::default()));
    push("port", "socket list", status, note);
    let (status, note) = classify_call(&context.port.owners(&Default::default()));
    push("port", "socket owners", status, note);
    push(
        "port",
        "kill by port",
        CapabilityStatus::Supported,
        Some("implemented; not exercised by this probe, privileges apply".to_string()),
    );

    // ---- net ----------------------------------------------------------------
    let interfaces = context.network.interfaces();
    let (status, note) = classify_call(&interfaces);
    push("net", "interfaces", status, note);
    if let Ok(rows) = &interfaces {
        let (status, note) = if rows.iter().any(|row| row.received_bytes.is_some()) {
            (CapabilityStatus::Supported, None)
        } else {
            (
                CapabilityStatus::Degraded,
                Some("no interface exposed traffic counters".to_string()),
            )
        };
        push("net", "traffic counters", status, note);
    }

    let addresses = context.network.addresses();
    let (status, note) = classify_call(&addresses);
    push("net", "addresses", status, note);
    if let Ok(rows) = &addresses {
        let (status, note) = if rows.iter().any(|row| row.dhcp.is_some()) {
            (CapabilityStatus::Supported, None)
        } else {
            (
                CapabilityStatus::Degraded,
                Some("the platform does not record the address source".to_string()),
            )
        };
        push("net", "dhcp marks", status, note);
    }

    let (status, note) = classify_call(&context.network.routes());
    push("net", "routes", status, note);
    let (status, note) = classify_call(&context.network.dns());
    push("net", "dns configuration", status, note);

    // Loopback answers instantly and needs no outbound network.
    let (status, note) = classify_call(&context.network.resolve("localhost"));
    push("net", "name resolution", status, note);
    let ping = context.network.ping(&PingRequest {
        address: IpAddr::V4(Ipv4Addr::LOCALHOST),
        count: 1,
        timeout_ms: 800,
    });
    let (status, note) = classify_call(&ping);
    push("net", "ping (loopback)", status, note);
    let trace = context
        .network
        .trace(IpAddr::V4(Ipv4Addr::LOCALHOST), 1, 800);
    let (status, note) = classify_call(&trace);
    push("net", "trace (loopback)", status, note);
    let (status, note) = classify_call(&context.network.flush_dns_cache());
    push("net", "dns cache flush", status, note);

    // ---- disk -----------------------------------------------------------------
    let disks = context.disk.list();
    let (status, note) = classify_call(&disks);
    push("disk", "mount list", status, note);
    if let Ok(rows) = &disks {
        let (status, note) = if rows.iter().any(|row| row.volume_uuid.is_some()) {
            (CapabilityStatus::Supported, None)
        } else {
            (
                CapabilityStatus::Degraded,
                Some("no volume exposed a platform-visible uuid".to_string()),
            )
        };
        push("disk", "volume / partition uuids", status, note);
        let (status, note) = if rows.iter().any(|row| row.media_type.is_some()) {
            (CapabilityStatus::Supported, None)
        } else {
            (
                CapabilityStatus::Degraded,
                Some("physical drive facts are not reachable".to_string()),
            )
        };
        push("disk", "physical drive facts", status, note);
    }
    push(
        "disk",
        "directory usage walk",
        CapabilityStatus::Supported,
        Some("pure filesystem walk, available on every platform".to_string()),
    );

    // ---- service --------------------------------------------------------------
    let manager = context.service.manager_type();
    push(
        "service",
        "service manager",
        if manager == ServiceManagerType::Unknown {
            CapabilityStatus::Unsupported
        } else {
            CapabilityStatus::Supported
        },
        Some(manager.name().to_string()),
    );
    let (status, note) = classify_call(&context.service.list(&Default::default()));
    push("service", "service list", status, note);
    push(
        "service",
        "service actions",
        if manager.supports_control() {
            CapabilityStatus::Supported
        } else {
            CapabilityStatus::Unsupported
        },
        Some("not exercised by this probe, privileges apply".to_string()),
    );

    // Deliberately invalid names: an adapter that can answer this question
    // rejects the input *after* checking whether the feature exists, so the
    // error kind tells support apart without reading anything.
    let logs = context.service.logs("a\"", None);
    let (status, note) = match &logs {
        Err(error) if error.kind() == ErrorKind::InvalidInput => {
            (CapabilityStatus::Supported, None)
        }
        _ => classify_call(&logs),
    };
    push("service", "service logs", status, note);
    let native = context.service.native(&[]);
    let (status, note) = match &native {
        Err(error) if error.kind() == ErrorKind::InvalidInput => {
            (CapabilityStatus::Supported, None)
        }
        _ => classify_call(&native),
    };
    push("service", "native escape hatch", status, note);

    out
}

/// Classify a probe that returns a `Result`.
fn classify_call<T>(outcome: &Result<T>) -> (CapabilityStatus, Option<String>) {
    match outcome {
        Ok(_) => (CapabilityStatus::Supported, None),
        Err(error) => classify_error(error),
    }
}

/// Map an error onto a status and a human note.
fn classify_error(error: &crate::error::Error) -> (CapabilityStatus, Option<String>) {
    match error.kind() {
        ErrorKind::Unsupported => (
            CapabilityStatus::Unsupported,
            Some(error.message().to_string()),
        ),
        ErrorKind::PermissionDenied => (
            CapabilityStatus::Degraded,
            Some(error.permission().map_or_else(
                || "requires elevated privileges".to_string(),
                |requirement| requirement.guidance().to_string(),
            )),
        ),
        _ => (
            CapabilityStatus::Degraded,
            Some(error.message().to_string()),
        ),
    }
}

/// `presence` with the honest degraded note for absent optional data.
fn presence<T>(value: Option<T>, why: &'static str) -> (CapabilityStatus, Option<&'static str>) {
    match value {
        Some(_) => (CapabilityStatus::Supported, None),
        None => (CapabilityStatus::Degraded, Some(why)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{stub_service, Stubs};

    #[test]
    fn every_probe_has_a_domain_and_a_feature() {
        let context = Stubs::new()
            .with_services(vec![stub_service("sshd", 42)])
            .context();
        let rows = probe(&context);
        assert!(!rows.is_empty());
        assert!(rows.iter().all(|row| !row.domain.is_empty()));
        assert!(rows.iter().all(|row| !row.feature.is_empty()));
        // The probe covers all six domains.
        for domain in ["system", "process", "port", "net", "disk", "service"] {
            assert!(
                rows.iter().any(|row| row.domain == domain),
                "no rows for {domain}"
            );
        }
    }

    #[test]
    fn stub_log_source_and_native_are_classified_from_their_answers() {
        let stubs = Stubs::new();
        let context = stubs.context();
        let rows = probe(&context);
        // The default stub answers `logs` with Unsupported...
        let logs = rows
            .iter()
            .find(|row| row.feature == "service logs")
            .expect("row");
        assert_eq!(logs.status, CapabilityStatus::Unsupported);
        // ...while its `native` answers Ok, which counts as a hatch.
        let native = rows
            .iter()
            .find(|row| row.feature == "native escape hatch")
            .expect("row");
        assert_eq!(native.status, CapabilityStatus::Supported);
    }

    #[test]
    fn status_names_match_the_json_spelling() {
        assert_eq!(CapabilityStatus::Supported.name(), "supported");
        assert_eq!(
            serde_json::to_string(&CapabilityStatus::Degraded).unwrap(),
            "\"degraded\""
        );
    }
}
