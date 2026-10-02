//! The macOS side of the SMC read: IOKit FFI driving the wire protocol that
//! lives in [`crate::common::smc`].
//!
//! Nothing about the SMC protocol is decided here — struct layout, command
//! codes, key packing and `sp78` decoding are all in the common module and
//! tested on every platform. This file is only the Mach plumbing, and it
//! exists mostly to carry the two ABI facts that a mistake in does not fail a
//! call but corrupts memory:
//!
//! - `IOConnectCallStructMethod` takes its input and output sizes as
//!   `size_t` (`usize`), not `u32`. The wrong width leaves the upper register
//!   half uninitialised; the kernel then copies the wrong number of bytes and
//!   the test binary dies with SIGBUS on the way out.
//! - `IOServiceGetMatchingService` **consumes** the reference to the matching
//!   dictionary it is handed, so there is deliberately no `CFRelease` of it
//!   here — releasing would be an over-release of memory we no longer own.
//!
//! Every failure path degrades to `None`: the service may be absent, the read
//! may be refused without privileges, and a virtualised runner may have an SMC
//! that answers for none of the CPU keys. "Nothing said" is the honest
//! answer for all of those, and none of them may crash.

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

// The task port for "this process": `mach_task_self()` in C is a macro for
// the exported `mach_task_self_` symbol, so that is what we link against.
extern "C" {
    fn mach_task_self_() -> MachPort;
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
        // SAFETY: `connection` is our own storage, filled in by the kernel.
        let rc = unsafe { IOServiceOpen(service, mach_task_self_(), 0, &mut connection) };
        // SAFETY: `service` came from `IOServiceGetMatchingService` and we hold
        // its one reference.
        unsafe { IOObjectRelease(service) };
        if rc != 0 {
            return None;
        }
        Some(Self { connection })
    }

    /// One 80-byte `SMCKeyData_t` round trip.
    ///
    /// Both the IOKit return code and the SMC's own `result` byte must say
    /// success; anything else means "this machine would not say".
    fn call(&self, input: &mut proto::SmcKeyData) -> Option<proto::SmcKeyData> {
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
        if rc != 0 || output.result != proto::SMC_RESULT_SUCCESS {
            return None;
        }
        Some(output)
    }

    /// Total number of keys the SMC holds, via the `#KEY` pseudo-key.
    fn key_count(&self) -> Option<u32> {
        let mut info_req = proto::SmcKeyData::default();
        info_req.key = proto::pack_key("#KEY");
        info_req.data8 = proto::SMC_CMD_READ_KEYINFO;
        let info = self.call(&mut info_req)?;
        let size = info.key_info.data_size;
        if size == 0 || size as usize > proto::SmcKeyData::default().bytes.len() {
            return None;
        }
        let mut read_req = proto::SmcKeyData::default();
        read_req.key = info_req.key;
        read_req.data8 = proto::SMC_CMD_READ_BYTES;
        read_req.key_info.data_size = size;
        let value = self.call(&mut read_req)?;
        proto::decode_key_count(&value.bytes)
    }

    /// The name of the key at `index`.
    fn key_name_at(&self, index: u32) -> Option<String> {
        let mut req = proto::SmcKeyData::default();
        req.data8 = proto::SMC_CMD_READ_INDEX;
        req.data32 = index;
        let out = self.call(&mut req)?;
        Some(proto::key_name(out.key))
    }

    /// Read one key's type and payload.
    fn read_key(&self, key: u32) -> Option<(u32, [u8; 32])> {
        let mut info_req = proto::SmcKeyData::default();
        info_req.key = key;
        info_req.data8 = proto::SMC_CMD_READ_KEYINFO;
        let info = self.call(&mut info_req)?;
        let size = info.key_info.data_size;
        if size == 0 || size as usize > info.bytes.len() {
            return None;
        }
        let mut read_req = proto::SmcKeyData::default();
        read_req.key = key;
        read_req.data8 = proto::SMC_CMD_READ_BYTES;
        read_req.key_info.data_size = size;
        let value = self.call(&mut read_req)?;
        Some((info.key_info.data_type, value.bytes))
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
    let count = smc.key_count()?;
    // A machine claiming an implausible key count is answering with something
    // other than a real SMC; enumerating it would be slow and meaningless.
    if count == 0 || count > 4096 {
        return None;
    }

    let mut hottest: Option<f32> = None;
    for index in 0..count {
        let Some(name) = smc.key_name_at(index) else {
            continue;
        };
        if !proto::is_cpu_temperature_key(&name) {
            continue;
        }
        let Some((data_type, bytes)) = smc.read_key(proto::pack_key(&name)) else {
            continue;
        };
        if data_type != proto::DATA_TYPE_SP78 {
            continue;
        }
        let Some(celsius) = proto::decode_sp78(&bytes) else {
            continue;
        };
        if hottest.is_none_or(|current| celsius > current) {
            hottest = Some(celsius);
        }
    }
    hottest
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
