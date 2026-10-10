//! Adapters shared by more than one platform.
//!
//! These still live in `x-platform`: only `x-core` must stay OS agnostic. The
//! difference is that these implementations rely on `sysinfo`, which already
//! speaks to every kernel we support, so there is no reason to duplicate them
//! per OS.

pub mod cpu_sysinfo;
pub mod disk_sysinfo;
pub mod identity;
pub mod process_sysinfo;

pub mod bluetooth_os;
pub mod clipboard_cmd;
pub mod device_os;
pub mod display_os;
pub mod file_open;
pub mod firewall_os;
pub mod hosts_os;
pub mod logs_os;
pub mod mount_os;
pub mod netdiag;
pub mod pathperm_os;
pub mod power_os;
pub mod proxy_cmd;
pub mod proxy_os;
pub mod schedule_os;
pub mod shell_os;
pub mod smc;
pub mod startup_os;
pub mod user_os;
pub mod window_os;

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub mod capture;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub mod capture_unix;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub mod ifaddrs;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub mod netprobe;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub mod packet;
