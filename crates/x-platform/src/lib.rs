//! `x-platform`: the only crate allowed to know what OS it is running on.
//!
//! ```text
//! x-cli / x-tui ──> x-core traits ──> x-platform ──> macOS / Linux / Windows
//! ```
//!
//! # Structure
//!
//! ```text
//! src/
//! ├── common/     adapters that work everywhere through `sysinfo`
//! ├── macos/      libproc, sysctl, getifaddrs, launchd
//! ├── linux/      procfs, netlink style syscalls, systemd / OpenRC / SysV
//! └── windows/    windows-sys: TCP tables, SCM, IP helper API
//! ```
//!
//! Exactly one family is compiled, selected by the target. [`create_context`]
//! is the single entry point; nothing above this crate ever sees an OS name.
//!
//! # Native first
//!
//! Each adapter documents which layer it uses:
//!
//! 1. Rust libraries (`sysinfo`, `procfs`, `core-foundation`).
//! 2. OS native APIs (`libproc`, `sysctl`, `GetExtendedTcpTable`, ...).
//! 3. OS commands (`launchctl`, `systemctl`, `scutil`) — only where the platform
//!    exposes no usable native API, never as the primary path.

pub mod audit;
pub mod common;
pub mod sys;

#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(target_os = "linux")]
pub mod linux;

#[cfg(windows)]
pub mod windows;

pub use sys::{current_user_name, host_name, is_linux, is_macos, is_windows};

use x_core::context::SystemContext;
use x_core::error::Result;

/// Name of the platform family this binary was compiled for.
pub const fn platform_name() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "macos"
    }
    #[cfg(target_os = "linux")]
    {
        "linux"
    }
    #[cfg(windows)]
    {
        "windows"
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        "unsupported"
    }
}

/// Build the [`SystemContext`] for the current platform.
///
/// This is the only place in the workspace that chooses an implementation, which
/// is why adding a frontend never requires touching platform code.
pub fn create_context() -> Result<SystemContext> {
    #[cfg(target_os = "macos")]
    {
        SystemContext::builder()
            .system(macos::system::manager())
            .process(macos::process::manager())
            .port(macos::port::manager())
            .network(macos::network::manager())
            .service(macos::service::manager())
            .disk(macos::disk::manager())
            .file(std::sync::Arc::new(common::file_open::PlatformFile))
            .clipboard(std::sync::Arc::new(
                common::clipboard_cmd::PlatformClipboard,
            ))
            .user(std::sync::Arc::new(common::user_os::PlatformUser))
            .shell(std::sync::Arc::new(common::shell_os::PlatformShell))
            .proxy(std::sync::Arc::new(common::proxy_cmd::PlatformProxy))
            .power(std::sync::Arc::new(common::power_os::PlatformPower))
            .mount(std::sync::Arc::new(common::mount_os::PlatformMount))
            .startup(std::sync::Arc::new(common::startup_os::PlatformStartup))
            .schedule(std::sync::Arc::new(common::schedule_os::PlatformSchedule))
            .firewall(std::sync::Arc::new(common::firewall_os::PlatformFirewall))
            .logs(std::sync::Arc::new(common::logs_os::PlatformLogs))
            .device(std::sync::Arc::new(common::device_os::PlatformDevices))
            .build()
    }

    #[cfg(target_os = "linux")]
    {
        SystemContext::builder()
            .system(linux::system::manager())
            .process(linux::process::manager())
            .port(linux::port::manager())
            .network(linux::network::manager())
            .service(linux::service::manager())
            .disk(linux::disk::manager())
            .file(std::sync::Arc::new(common::file_open::PlatformFile))
            .clipboard(std::sync::Arc::new(
                common::clipboard_cmd::PlatformClipboard,
            ))
            .user(std::sync::Arc::new(common::user_os::PlatformUser))
            .shell(std::sync::Arc::new(common::shell_os::PlatformShell))
            .proxy(std::sync::Arc::new(common::proxy_cmd::PlatformProxy))
            .power(std::sync::Arc::new(common::power_os::PlatformPower))
            .mount(std::sync::Arc::new(common::mount_os::PlatformMount))
            .startup(std::sync::Arc::new(common::startup_os::PlatformStartup))
            .schedule(std::sync::Arc::new(common::schedule_os::PlatformSchedule))
            .firewall(std::sync::Arc::new(common::firewall_os::PlatformFirewall))
            .logs(std::sync::Arc::new(common::logs_os::PlatformLogs))
            .device(std::sync::Arc::new(common::device_os::PlatformDevices))
            .build()
    }

    #[cfg(windows)]
    {
        SystemContext::builder()
            .system(windows::system::manager())
            .process(windows::process::manager())
            .port(windows::port::manager())
            .network(windows::network::manager())
            .service(windows::service::manager())
            .disk(windows::disk::manager())
            .file(std::sync::Arc::new(common::file_open::PlatformFile))
            .clipboard(std::sync::Arc::new(
                common::clipboard_cmd::PlatformClipboard,
            ))
            .user(std::sync::Arc::new(common::user_os::PlatformUser))
            .shell(std::sync::Arc::new(common::shell_os::PlatformShell))
            .proxy(std::sync::Arc::new(common::proxy_cmd::PlatformProxy))
            .power(std::sync::Arc::new(common::power_os::PlatformPower))
            .mount(std::sync::Arc::new(common::mount_os::PlatformMount))
            .startup(std::sync::Arc::new(common::startup_os::PlatformStartup))
            .schedule(std::sync::Arc::new(common::schedule_os::PlatformSchedule))
            .firewall(std::sync::Arc::new(common::firewall_os::PlatformFirewall))
            .logs(std::sync::Arc::new(common::logs_os::PlatformLogs))
            .device(std::sync::Arc::new(common::device_os::PlatformDevices))
            .build()
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        Err(x_core::Error::unsupported(format!(
            "x has no adapter for this platform ({}); supported: macos, linux, windows",
            std::env::consts::OS
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use x_core::port::PortListOptions;

    #[test]
    fn platform_name_matches_the_build_target() {
        let expected = if cfg!(target_os = "macos") {
            "macos"
        } else if cfg!(target_os = "linux") {
            "linux"
        } else if cfg!(windows) {
            "windows"
        } else {
            "unsupported"
        };
        assert_eq!(platform_name(), expected);
    }

    #[test]
    fn context_builds_with_every_capability() {
        let context = create_context().expect("context");
        assert!(context.system.info().is_ok());
        assert!(context
            .process
            .list(&x_core::ProcessListOptions::light())
            .is_ok());
        assert!(context.port.list(&PortListOptions::default()).is_ok());
        assert!(context.network.interfaces().is_ok());
        assert!(context.disk.list().is_ok());
    }

    #[test]
    fn listening_snapshot_includes_this_process_server() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();

        let context = create_context().expect("context");
        let rows = context.port.listening().expect("listening");
        assert!(
            rows.iter().any(|r| r.local_port == port),
            "port {port} missing from {rows:?}"
        );
    }
}
