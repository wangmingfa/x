//! Linux adapters: `/proc` for sockets and routes, `systemd`/`OpenRC`/`SysV` for
//! services, `sysinfo` for processes and disks.
//!
//! Layer by layer, as documented at the crate root:
//!
//! 1. `sysinfo` for processes, disks and CPU.
//! 2. `/proc/net/{tcp,tcp6,udp,udp6}` plus `/proc/<pid>/fd` symlinks for
//!    process/socket attribution, `/proc/net/route` and `/proc/net/ipv6_route`
//!    for the routing table, `/etc/resolv.conf` for DNS. No process is spawned
//!    for any of it.
//! 3. `systemctl`, `rc-service` and `service` only for service lifecycle, which
//!    has no stable native interface outside the init system itself.

pub mod disk;
pub mod network;
pub mod port;
pub mod process;
pub mod service;
pub mod system;
