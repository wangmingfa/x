//! macOS adapter family.
//!
//! Layer order, per the native-first policy:
//!
//! 1. `libproc` / `sysctl` / Mach syscalls for processes, sockets and routes.
//! 2. `getifaddrs` for interfaces and addresses.
//! 3. `scutil` only for the resolver configuration, which has no stable
//!    public C API.

pub mod disk;
pub mod libproc;
pub mod network;
pub mod port;
pub mod process;
pub mod service;
pub mod smc;
pub mod system;
