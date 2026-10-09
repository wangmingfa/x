//! The macOS side of the SMC read: IOKit FFI driving the wire protocol that
//! lives in [`crate::common::smc`].
//!
//! Nothing about the SMC protocol is decided here — struct layout, command
//! codes, key packing, the per-type temperature decoders, the rule that refuses
//! an undifferentiated group of readings and the read loop that ties them
//! together are all in the common module, replayed against real hardware
//! transcripts and tested on every platform. This file is the Mach plumbing and
//! nothing else: open the user client, answer one 80-byte call, close it on
//! drop. It exists mostly to carry the ABI facts that a mistake in does not
//! fail a call but corrupts memory:
//!
//! - `IOConnectCallStructMethod` takes its input and output sizes as
//!   `size_t` (`usize`), not `u32`. The wrong width leaves the upper register
//!   half uninitialised; the kernel then copies the wrong number of bytes and
//!   the test binary dies with SIGBUS on the way out.
//! - `mach_task_self_` is a **data symbol** holding the current task's port —
//!   `mach_task_self()` in C is a macro that *reads* it. Declaring it as a
//!   function still links, then jumps into `__DATA`: the first four bytes
//!   there are the port value itself, which is not an instruction.
//! - `IOServiceGetMatchingService` **consumes** the reference to the matching
//!   dictionary it is handed, so there is deliberately no `CFRelease` of it
//!   here — releasing would be an over-release of memory we no longer own.
//!
//! Every failure path degrades to `None`: the service may be absent, the read
//! may be refused without privileges, a virtualised runner may have an SMC that
//! answers for none of the CPU keys, and a sensor may carry a type x does not
//! decode. "Nothing said" is the honest answer for all of those, and none of
//! them may crash.

use crate::common::smc as proto;
use std::os::raw::{c_char, c_void};
use x_core::error::Result;

// IOKit handles are mach ports: `u32` on 64-bit macOS.
type IoObject = u32;
type IoConnect = u32;
type MachPort = u32;
type KernReturn = i32;

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOServiceMatching(name: *const c_char) -> *mut c_void;
    fn IOServiceGetMatchingService(main_port: MachPort, matching: *mut c_void) -> IoObject;
    fn IOServiceOpen(
        service: IoObject,
        task: MachPort,
        options: u32,
        connect: *mut IoConnect,
    ) -> KernReturn;
    fn IOConnectCallStructMethod(
        connection: IoConnect,
        selector: u32,
        input: *const c_void,
        input_size: usize,
        output: *mut c_void,
        output_size: *mut usize,
    ) -> KernReturn;
    fn IOServiceClose(connection: IoConnect) -> KernReturn;
    fn IOObjectRelease(object: IoObject) -> KernReturn;
}

// The task port for "this process". `mach_task_self_` is a variable exported
// by libsystem_kernel, so it is declared as one and read, never called.
extern "C" {
    static mach_task_self_: MachPort;
}

/// `kIOMainPortDefault` (historically `kIOMasterPortDefault`): the current
/// task, expressed as the port value 0.
const MAIN_PORT_DEFAULT: MachPort = 0;

/// An open connection to the `AppleSMC` user client, closed on drop.
struct Smc {
    connection: IoConnect,
}

impl Smc {
    /// Open the user client, or `None` when the service is missing or the
    /// open is refused.
    fn open() -> Option<Self> {
        let name = std::ffi::CString::new("AppleSMC").ok()?;
        // SAFETY: `name` is NUL terminated and lives for the call.
        let matching = unsafe { IOServiceMatching(name.as_ptr()) };
        if matching.is_null() {
            return None;
        }
        // SAFETY: `matching` is a valid dictionary; the call consumes our
        // reference to it, so it is never released below.
        let service = unsafe { IOServiceGetMatchingService(MAIN_PORT_DEFAULT, matching) };
        if service == 0 {
            return None;
        }
        let mut connection: IoConnect = 0;
        // SAFETY: `connection` is our own storage, filled in by the kernel, and
        // `mach_task_self_` is the process-wide task port libsystem_kernel set.
        let rc = unsafe { IOServiceOpen(service, mach_task_self_, 0, &mut connection) };
        // SAFETY: `service` came from `IOServiceGetMatchingService` and we hold
        // its one reference.
        unsafe { IOObjectRelease(service) };
        if rc != 0 {
            return None;
        }
        Some(Self { connection })
    }
}

impl proto::RoundTrip for Smc {
    /// One 80-byte `SMCKeyData_t` round trip over the user client.
    ///
    /// Only the transport's own failure is reported here (a kern return that is
    /// not success); the SMC's `result` byte is read by
    /// [`proto::SmcClient::call`], so a replay can produce either kind of
    /// failure and the protocol code sees the same shape either way.
    fn round_trip(&self, input: &proto::SmcKeyData) -> Option<proto::SmcKeyData> {
        let mut output = proto::SmcKeyData::default();
        let size = std::mem::size_of::<proto::SmcKeyData>();
        let mut out_size = size;
        // SAFETY: both structures are our own, fully initialised, and exactly
        // `size_of::<SmcKeyData>()` (80) bytes — asserted at compile time in
        // the common module. The sizes are `usize`, the ABI this API actually
        // takes; getting that width wrong is the SIGBUS this file exists to
        // prevent.
        let rc = unsafe {
            IOConnectCallStructMethod(
                self.connection,
                proto::SMC_SELECTOR,
                input as *const proto::SmcKeyData as *const c_void,
                size,
                &mut output as *mut proto::SmcKeyData as *mut c_void,
                &mut out_size,
            )
        };
        if rc != 0 {
            return None;
        }
        Some(output)
    }
}

impl Drop for Smc {
    fn drop(&mut self) {
        // SAFETY: `connection` is the handle `IOServiceOpen` handed us, and
        // drop runs exactly once.
        unsafe { IOServiceClose(self.connection) };
    }
}

/// The hottest CPU temperature this machine reports, in degrees Celsius.
///
/// `Ok(None)` means the SMC could not be opened, the read was refused, or no
/// CPU sensor answered — all rendered downstream as "not exposed", none
/// inferred from.
pub fn cpu_temperature() -> Result<Option<f32>> {
    Ok(read_cpu_temperature())
}

fn read_cpu_temperature() -> Option<f32> {
    let smc = Smc::open()?;
    // Everything past the open — counting, enumerating, filtering, the per-type
    // decode and the group judgement — is [`proto::SmcClient`], replayed against
    // real transcripts in the common module on every platform.
    proto::SmcClient::new(smc).cpu_temperature()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_read_degrades_without_panicking() {
        // On a runner whose SMC is missing or refuses the read this is None;
        // on a real Mac it is a plausible die temperature. Either answer is
        // fine — the regression this guards is the SIGBUS that used to come
        // out of this call path instead.
        if let Some(celsius) = read_cpu_temperature() {
            assert!(
                (proto::MIN_PLAUSIBLE_CELSIUS..=proto::MAX_PLAUSIBLE_CELSIUS).contains(&celsius),
                "{celsius} is not a plausible CPU temperature"
            );
        }
    }
}
