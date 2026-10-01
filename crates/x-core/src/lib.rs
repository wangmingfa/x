//! `x-core`: the unified system model shared by every frontend and every platform.
//!
//! # The one rule
//!
//! `x-core` does **not** know that Windows, Linux and macOS exist. There is not
//! a single platform conditional cfg in this crate. It only knows
//! *capabilities* ([`ProcessManager`], [`PortManager`], [`ServiceManager`], ...),
//! the *models* those capabilities speak and the *errors* they can return.
//!
//! ```text
//! x-cli ─┐
//! x-tui ─┴─> SystemContext (dyn traits + unified models) ─> x-platform ─> OS
//! ```
//!
//! Consequence: adding a GUI, an HTTP API, an MCP server or a plugin later
//! only means "implement a frontend on top of the same [`SystemContext`]".
//!
//! # Snapshot, never watch
//!
//! Traits only expose snapshot style reads (`list`, `info`, `cpu_usage`).
//! Watching is built on top of polling in the frontend, so `x process list`
//! and the live TUI refresh execute exactly the same code.
//!
//! # Example
//!
//! ```
//! use x_core::port::{PortQuery, PortSort};
//! use x_core::testing::stub_context;
//!
//! # fn main() -> x_core::error::Result<()> {
//! // A real binary gets this from `x_platform::create_context()`. Tests use stubs.
//! let context = stub_context();
//! let plan = context.port.plan(&PortQuery::Port(8080), PortSort::Port)?;
//! println!("{} socket(s) hold 8080", plan.sockets.len());
//! # Ok(())
//! # }
//! ```

pub mod audit;
pub mod bluetooth;
pub mod capability;
pub mod clipboard;
pub mod context;
pub mod devcheck;
pub mod devenv;
pub mod device;
pub mod disk;
pub mod display;
pub mod dockerinfo;
pub mod error;
pub mod file;
pub mod firewall;
pub mod gitcmd;
pub mod hostsfile;
pub mod logs;
pub mod mount;
pub mod netdiag;
pub mod network;
pub mod pathperm;
pub mod port;
pub mod power;
pub mod process;
pub mod project;
pub mod proxy;
pub mod schedule;
pub mod service;
pub mod shell;
pub mod sshcfg;
pub mod startup;
pub mod system;
pub mod testing;
pub mod user;

pub use audit::{now_utc_rfc3339, rfc3339_utc, AuditEntry};
pub use capability::{probe as probe_capabilities, Capability, CapabilityStatus};
pub use context::{SystemContext, SystemContextBuilder};
pub use disk::{walk_directory, DirUsage, DiskInfo, DiskManager, MediaType};
pub use error::{Error, ErrorKind, PermissionRequirement, Result, ResultExt};
pub use network::{
    AddressInfo, DnsConfig, DnsServer, InterfaceInfo, InterfaceState, NetworkManager, RouteInfo,
};
pub use port::{
    ConnectionState, KillPlan, PortInfo, PortListOptions, PortManager, PortOwner, PortQuery,
    PortSort, Protocol,
};
pub use process::{
    KillSignal, ProcessInfo, ProcessListOptions, ProcessManager, ProcessNode, ProcessSort,
    ProcessState, ProcessTree,
};
pub use service::{
    NativeOutput, ServiceAction, ServiceInfo, ServiceListOptions, ServiceLogEntry, ServiceLogPage,
    ServiceManager, ServiceManagerType, ServiceState,
};
pub use system::{
    format_bytes, format_duration, format_timestamp, percent, CpuUsage, LoadAverage, MemoryUsage,
    OsFamily, PressureLevel, SystemInfo, SystemManager,
};

/// Version of the `x-core` contract, useful for plugin compatibility checks.
pub const CONTRACT_VERSION: u32 = 1;

/// Architectural guard: `x-core` must never learn about the host OS.
///
/// If a future contributor adds a platform conditional cfg here, this test
/// fails and the dependency direction is restored immediately.
#[cfg(test)]
mod architecture_tests {
    use std::path::{Path, PathBuf};

    fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    #[test]
    fn core_has_no_os_specific_cfg() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let src = manifest.join("src");
        let mut files = Vec::new();
        sources(&src, &mut files);
        assert!(
            !files.is_empty(),
            "no sources found under {}",
            src.display()
        );

        // Markers are assembled at runtime so this guard file does not trip itself.
        let os_marker = format!("target{}", "_os");
        let family_marker = format!("target{}", "_family");
        let win_marker = format!("cfg({})", "windows");
        let unix_marker = format!("cfg({})", "unix");

        for file in files {
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            for marker in [
                os_marker.as_str(),
                family_marker.as_str(),
                &win_marker,
                &unix_marker,
            ] {
                assert!(
                    !text.contains(marker),
                    "{} references `{marker}`: platform logic belongs in x-platform, \
                     x-core only speaks capabilities",
                    file.display()
                );
            }
        }
    }

    #[test]
    fn core_does_not_depend_on_platform_crates() {
        let manifest =
            std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
                .expect("read Cargo.toml");
        for forbidden in ["sysinfo", "libc", "windows-sys", "nix", "procfs"] {
            assert!(
                !manifest.contains(forbidden),
                "x-core must not depend on `{forbidden}`: keep the model OS agnostic"
            );
        }
    }
}
