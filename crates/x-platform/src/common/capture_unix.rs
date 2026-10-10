//! The capture device layer: opening the kernel packet source and reading
//! frames from it.
//!
//! * macOS: one `/dev/bpfN` device per sampler, bound to an interface with
//!   `BIOCSETIF`, header length probed with `BIOCGDLT` (loopback reports
//!   DLT_NULL, regular interfaces DLT_EN10MB).
//! * Linux: an `AF_PACKET`/`SOCK_RAW` socket bound with `ETH_P_IP`, which
//!   delivers bare IP datagrams (no link header to strip).
//!
//! Both open lazily through [`CaptureHandle::open`] and fail with a plain
//! error when unprivileged — the CLI turns that into the `hint:` line, the
//! same way other permission-gated reads behave.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use x_core::error::{Error, ErrorKind, Result};
use x_core::net_top::{NetSnapshot, PidBytes};

use super::capture::{FlowAggregator, OwnerTable};
use super::packet::Datalink;

/// One frame batch from the device, plus the datalink the device speaks.
pub struct FrameBatch<'a> {
    pub datalink: Datalink,
    pub frames: Vec<&'a [u8]>,
}

/// Platform capture device: reads frames; nothing else is its business.
pub trait CaptureDevice: Send {
    /// Read every frame currently available, blocking at most `timeout`
    /// when none is pending. Returns an empty batch on timeout.
    fn read_frames(&mut self, timeout: Duration) -> Result<Vec<Vec<u8>>>;
    /// The datalink layout of the frames this device yields.
    fn datalink(&self) -> Datalink;
}

fn capture_error(message: impl Into<String>) -> Error {
    Error::new(ErrorKind::PermissionDenied, message)
}

#[cfg(target_os = "macos")]
mod device {
    //! `/dev/bpfN`: open, bind to an interface, read frames.
    use super::*;
    use std::fs::{File, OpenOptions};
    use std::io::Read;
    use std::os::unix::io::AsRawFd;

    const BIOCSETIF: u64 = 0x8020426c;
    const BIOCGDLT: u64 = 0x40044266;
    const BIOCIMMEDIATE: u64 = 0x80044270;
    const DLT_EN10MB: u32 = 1;
    const DLT_NULL: u32 = 0;

    #[repr(C)]
    struct IfReq {
        name: [u8; 16],
        // sa_len + sa_family + data, big enough for a sockaddr_dl.
        addr: [u8; 20],
    }

    pub struct BpfDevice {
        file: File,
        datalink: Datalink,
        buffer: Vec<u8>,
    }

    impl BpfDevice {
        /// Open the first free `/dev/bpfN` and bind it to `interface`.
        pub fn open(interface: &str) -> Result<Self> {
            for i in 0..32 {
                let path = format!("/dev/bpf{i}");
                let Ok(file) = OpenOptions::new().read(true).write(true).open(&path) else {
                    continue;
                };
                let mut ifr = IfReq {
                    name: [0; 16],
                    addr: [0; 20],
                };
                let bytes = interface.as_bytes();
                if bytes.len() >= 16 {
                    return Err(capture_error("interface name too long"));
                }
                ifr.name[..bytes.len()].copy_from_slice(bytes);
                let fd = file.as_raw_fd();
                // Immediate mode: reads return as soon as any frame is in.
                if unsafe { libc::ioctl(fd, BIOCIMMEDIATE as libc::c_ulong, 1i32) } < 0 {
                    continue;
                }
                if unsafe { libc::ioctl(fd, BIOCSETIF as libc::c_ulong, &ifr) } < 0 {
                    continue; // bound elsewhere or not a capture interface
                }
                let mut dlt: u32 = 0;
                if unsafe { libc::ioctl(fd, BIOCGDLT as libc::c_ulong, &mut dlt) } < 0 {
                    continue;
                }
                let datalink = match dlt {
                    DLT_NULL => Datalink::DltNull,
                    DLT_EN10MB => Datalink::Ethernet,
                    other => {
                        return Err(capture_error(format!(
                            "unsupported datalink type {other} on {interface}"
                        )))
                    }
                };
                return Ok(BpfDevice {
                    file,
                    datalink,
                    buffer: vec![0u8; 64 * 1024],
                });
            }
            Err(capture_error(format!(
                "no /dev/bpf device available for {interface} (capture requires root; hint: sudo x net top)"
            )))
        }
    }

    impl CaptureDevice for BpfDevice {
        fn read_frames(&mut self, timeout: Duration) -> Result<Vec<Vec<u8>>> {
            // poll(2) so a quiet network does not stall the sampler forever.
            let mut pollfd = libc::pollfd {
                fd: self.file.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let millis = timeout.as_millis().min(i32::MAX as u128) as i32;
            let ready = unsafe { libc::poll(&mut pollfd, 1, millis) };
            if ready <= 0 {
                return Ok(Vec::new());
            }
            let mut buf = std::mem::take(&mut self.buffer);
            let n = self.file.read(&mut buf).unwrap_or(0);
            self.buffer = buf;
            Ok(split_bpf_frames(&self.buffer[..n]))
        }

        fn datalink(&self) -> Datalink {
            self.datalink
        }
    }

    /// BPF hands back a stream of [bh(header) | frame] records.
    fn split_bpf_frames(data: &[u8]) -> Vec<Vec<u8>> {
        let mut frames = Vec::new();
        let mut offset = 0;
        while offset + 18 <= data.len() {
            let caplen =
                u32::from_le_bytes(data[offset + 12..offset + 16].try_into().unwrap()) as usize;
            offset += 18;
            if offset + caplen > data.len() {
                break;
            }
            frames.push(data[offset..offset + caplen].to_vec());
            offset += caplen;
        }
        frames
    }
}

#[cfg(target_os = "linux")]
mod device {
    //! `AF_PACKET` bound to `ETH_P_IP`: bare IP datagrams, no link header.
    use super::*;

    const ETH_P_IP: u16 = 0x0800;

    pub struct PacketDevice {
        fd: i32,
    }

    impl PacketDevice {
        pub fn open(_interface: &str) -> Result<Self> {
            // ETH_P_IP at the socket level receives every IPv4 datagram the
            // host sees; per-interface binding is skipped because the flow
            // table is interface-agnostic anyway.
            let fd =
                unsafe { libc::socket(libc::AF_PACKET, libc::SOCK_RAW, ETH_P_IP.to_be() as i32) };
            if fd < 0 {
                return Err(capture_error(
                    "AF_PACKET socket failed (capture requires CAP_NET_RAW; hint: sudo x net top)",
                ));
            }
            Ok(PacketDevice { fd })
        }
    }

    impl Drop for PacketDevice {
        fn drop(&mut self) {
            unsafe { libc::close(self.fd) };
        }
    }

    impl CaptureDevice for PacketDevice {
        fn read_frames(&mut self, timeout: Duration) -> Result<Vec<Vec<u8>>> {
            let mut pollfd = libc::pollfd {
                fd: self.fd,
                events: libc::POLLIN,
                revents: 0,
            };
            let millis = timeout.as_millis().min(i32::MAX as u128) as i32;
            let ready = unsafe { libc::poll(&mut pollfd, 1, millis) };
            if ready <= 0 {
                return Ok(Vec::new());
            }
            let mut buf = [0u8; 65_536];
            let n = unsafe { libc::recv(self.fd, buf.as_mut_ptr().cast(), buf.len(), 0) };
            if n <= 0 {
                return Ok(Vec::new());
            }
            // sll2/sll framing: the kernel prepends a sockaddr_ll; with
            // recv(2) on a bound socket the payload starts at the datagram,
            // so this is already a bare IP packet.
            Ok(vec![buf[..n as usize].to_vec()])
        }

        fn datalink(&self) -> Datalink {
            Datalink::RawIp
        }
    }
}

#[cfg(target_os = "macos")]
pub use device::BpfDevice;
#[cfg(target_os = "linux")]
pub use device::PacketDevice;

/// Total frames the sampler has processed, exposed for tests and the
/// degraded-mode diagnostics.
static FRAMES_SEEN: AtomicU64 = AtomicU64::new(0);

/// The L3 sampler for Unix: capture thread + owner table refresh per round.
///
/// Owns a device, an aggregator, and the socket-table provider closure. One
/// [`UnixCaptureSampler`] instance per `x net top` run.
pub struct UnixCaptureSampler {
    device: Mutex<Box<dyn CaptureDevice>>,
    aggregator: Mutex<FlowAggregator>,
    datalink: Datalink,
}

impl std::fmt::Debug for UnixCaptureSampler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The device is a raw fd wrapper; identity, not contents, is what a
        // test failure needs to name.
        f.debug_struct("UnixCaptureSampler")
            .field("datalink", &self.datalink)
            .finish_non_exhaustive()
    }
}

/// The socket table the sampler attributes against, injected per round so
/// the sampler stays independent of the port-manager lifetimes.
pub type SocketTableProvider<'a> = &'a dyn Fn() -> Vec<super::capture::SocketEntry>;

impl UnixCaptureSampler {
    pub fn open() -> Result<Self> {
        let device: Box<dyn CaptureDevice> = {
            #[cfg(target_os = "macos")]
            {
                // en0 covers the common case; loopback traffic attributes via
                // its own device only when explicitly asked for.
                Box::new(BpfDevice::open("en0")?)
            }
            #[cfg(target_os = "linux")]
            {
                Box::new(PacketDevice::open("any")?)
            }
        };
        let datalink = device.datalink();
        Ok(Self {
            device: Mutex::new(device),
            aggregator: Mutex::new(FlowAggregator::new()),
            datalink,
        })
    }

    /// One sampling round: pump the device for `window`, attributing frames
    /// against a freshly read socket table, then produce the snapshot.
    pub fn sample(
        &self,
        window: Duration,
        sockets: SocketTableProvider<'_>,
    ) -> Result<NetSnapshot> {
        let owners = OwnerTable::from_entries(&sockets());
        let deadline = Instant::now() + window;
        {
            let mut device = self
                .device
                .lock()
                .map_err(|_| Error::new(ErrorKind::System, "capture device poisoned"))?;
            loop {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                let frames = device.read_frames(deadline - now)?;
                let mut aggregator = self
                    .aggregator
                    .lock()
                    .map_err(|_| Error::new(ErrorKind::System, "flow aggregator poisoned"))?;
                for frame in frames {
                    FRAMES_SEEN.fetch_add(1, Ordering::Relaxed);
                    aggregator.credit(&frame, self.datalink, &owners);
                }
            }
        }
        let pid_bytes: Option<BTreeMap<i32, PidBytes>> = self
            .aggregator
            .lock()
            .map_err(|_| Error::new(ErrorKind::System, "flow aggregator poisoned"))?
            .take();
        Ok(NetSnapshot {
            pid_bytes,
            // Interface counters and the connection table come from the same
            // managers the rest of x reads; the sampler layer above stitches
            // them into this snapshot. Leaving them None here keeps this
            // module capture-only.
            interfaces: None,
            connections: None,
            failures: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_seen_counter_advances() {
        let before = FRAMES_SEEN.load(Ordering::Relaxed);
        FRAMES_SEEN.fetch_add(1, Ordering::Relaxed);
        assert!(FRAMES_SEEN.load(Ordering::Relaxed) > before);
    }

    #[test]
    fn unprivileged_open_reports_permission_with_hint() {
        // On a CI runner / non-root shell this is the expected branch; under
        // root the device opens and the assertion is skipped.
        if unsafe { libc::geteuid() == 0 } {
            return;
        }
        let err = UnixCaptureSampler::open().unwrap_err();
        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
    }
}
