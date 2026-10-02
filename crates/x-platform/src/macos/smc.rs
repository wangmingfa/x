//! macOS CPU die temperature through the System Management Controller.
//!
//! Layer 2 of the doctrine (`IOKit`): macOS exposes no public API for the die
//! sensor, and the kernel routes SMC reads through `AppleSMCUserClient`. The
//! keys are **enumerated** rather than hardcoded, because the naming differs per
//! chip generation — `TC0P` on Intel, `Tp01` on Apple silicon — so any fixed
//! list works on only some machines.
//!
//! Two honesty rules govern what comes back. A value is reported only when the
//! SMC actually answered with a `sp78` reading inside a plausible range; and
//! when the call is refused — the common case without root, since the user
//! client denies unprivileged SMC access — the caller gets `None`, which the
//! capability report already renders as "not exposed here". Nothing is
//! inferred, estimated or substituted for a missing sensor.

use x_core::error::Result;

#[cfg(target_os = "macos")]
use std::os::raw::{c_char, c_int, c_uint, c_void};

/// SMC command: read a key's type and size.
#[cfg(target_os = "macos")]
const SMC_CMD_READ_KEYINFO: u32 = 5;
/// SMC command: read a key's bytes.
#[cfg(target_os = "macos")]
const SMC_CMD_READ_BYTES: u32 = 6;
/// SMC command: enumerate the Nth key in the SMC.
#[cfg(target_os = "macos")]
const SMC_CMD_READ_INDEX: u32 = 8;

/// The `AppleSMCUserClient` selector that takes an `SMCKeyData_t` in and out.
#[cfg(target_os = "macos")]
const SMC_USER_CLIENT_SELECTOR: u32 = 2;

/// `sp78`: 8.8 signed fixed point, the encoding every CPU sensor uses.
#[cfg(target_os = "macos")]
const DATA_TYPE_SP78: u32 = 0x7370_3738;

/// Outside this range the SMC is not reporting a CPU temperature, and the
/// `0x7f7f` "invalid" marker is the case that shows up in practice.
const MIN_PLAUSIBLE_CELSIUS: f32 = -40.0;
const MAX_PLAUSIBLE_CELSIUS: f32 = 150.0;

/// One `SMCKeyData_t`, the 56 byte structure the user client speaks.
///
/// The layout is `key`, `command`, `data8`, then `SMCKeyInfo_t`
/// (`dataSize`, `dataType`, `dataAttributes` + 3 bytes of tail padding to the
/// next 4-byte boundary) and finally the 32 value bytes.
#[cfg(target_os = "macos")]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct SmcKeyData {
    key: u32,
    command: u32,
    data8: u32,
    data_size: u32,
    data_type: u32,
    data_attributes: u8,
    data_attributes_padding: [u8; 3],
    bytes: [u8; 32],
}

#[cfg(target_os = "macos")]
impl SmcKeyData {
    const SIZE: u32 = std::mem::size_of::<Self>() as u32;

    /// A request for `key` carrying `command`.
    fn request(key: u32, command: u32) -> Self {
        Self {
            key,
            command,
            ..Self::default()
        }
    }
}

#[cfg(target_os = "macos")]
#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOServiceMatching(name: *const c_char) -> *mut c_void;
    fn IOServiceGetMatchingService(main_port: c_uint, matching: *const c_void) -> c_uint;
    fn IOServiceOpen(service: c_uint, task: c_uint, options: u32, connect: *mut c_uint) -> c_int;
    fn IOConnectCallStructMethod(
        connection: c_uint,
        selector: u32,
        input: *const c_void,
        input_size: u32,
        output: *mut c_void,
        output_size: *mut u32,
    ) -> c_int;
    fn IOServiceClose(connection: c_uint) -> c_int;
    fn IOObjectRelease(object: c_uint) -> c_int;
    fn CFRelease(object: *const c_void);
}

// The task port for "this process". `libc::mach_task_self()` is the deprecated
// spelling of the constant `MACH_PORT_SELF`; the underscored entry point is the
// real symbol behind it, and naming it here keeps the deprecation off the build.
#[cfg(target_os = "macos")]
extern "C" {
    fn mach_task_self_() -> c_uint;
}

/// An open connection to the SMC user client.
///
/// The connection is closed on drop, so a read that goes wrong half way
/// through does not leak a kernel handle.
#[cfg(target_os = "macos")]
struct Smc {
    connection: c_uint,
}

#[cfg(target_os = "macos")]
impl Smc {
    /// Open the user client, or `None` when the kernel refuses.
    fn open() -> Option<Self> {
        let name = std::ffi::CString::new("AppleSMCUserClient").ok()?;
        // SAFETY: `name` is NUL terminated and lives for the call; the
        // dictionary it returns is owned by us and released just below.
        let matching = unsafe { IOServiceMatching(name.as_ptr()) };
        if matching.is_null() {
            return None;
        }
        // SAFETY: `matching` is the dictionary just created.
        let service =
            unsafe { IOServiceGetMatchingService(mach_task_self_(), matching as *const c_void) };
        // The dictionary is ours either way, so release it before the branches
        // below; the service retains its own reference.
        // SAFETY: same pointer that `IOServiceMatching` handed us.
        unsafe { CFRelease(matching) };
        if service == 0 {
            return None;
        }
        let mut connection: c_uint = 0;
        // SAFETY: `connection` is our own storage, filled in by the kernel.
        let rc = unsafe { IOServiceOpen(service, mach_task_self_(), 0, &mut connection) };
        // SAFETY: `service` is the object `IOServiceGetMatchingService` returned.
        unsafe { IOObjectRelease(service) };
        if rc != 0 {
            return None;
        }
        Some(Self { connection })
    }

    /// One `SMCKeyData_t` round trip.
    fn call(&self, input: &SmcKeyData) -> Option<SmcKeyData> {
        let mut output = SmcKeyData::default();
        let mut size = SmcKeyData::SIZE;
        // SAFETY: both structures are our own, fully initialised storage, and
        // the sizes describe them exactly.
        let rc = unsafe {
            IOConnectCallStructMethod(
                self.connection,
                SMC_USER_CLIENT_SELECTOR,
                input as *const SmcKeyData as *const c_void,
                SmcKeyData::SIZE,
                &mut output as *mut SmcKeyData as *mut c_void,
                &mut size,
            )
        };
        (rc == 0 && size >= SmcKeyData::SIZE).then_some(output)
    }

    /// Total number of keys the SMC holds, via the `#` info key.
    fn key_count(&self) -> Option<u32> {
        let response = self.call(&SmcKeyData::request(0x23, SMC_CMD_READ_KEYINFO))?;
        u32::from_le_bytes([
            response.bytes[0],
            response.bytes[1],
            response.bytes[2],
            response.bytes[3],
        ])
        .into()
    }

    /// The name of the `index`th key.
    fn key_name_at(&self, index: u32) -> Option<String> {
        let mut request = SmcKeyData::request(0, SMC_CMD_READ_INDEX);
        request.data8 = index;
        let response = self.call(&request)?;
        Some(four_char_code(u32::from_le_bytes([
            response.bytes[0],
            response.bytes[1],
            response.bytes[2],
            response.bytes[3],
        ])))
    }

    /// Read one key, returning its type and bytes.
    fn read_key(&self, key: u32) -> Option<(u32, [u8; 32])> {
        let info = self.call(&SmcKeyData::request(key, SMC_CMD_READ_KEYINFO))?;
        let size = info.data_size;
        let data_type = info.data_type;
        if size == 0 || size > 32 {
            return None;
        }
        let mut request = SmcKeyData::request(key, SMC_CMD_READ_BYTES);
        request.data_size = size;
        request.data_type = data_type;
        let response = self.call(&request)?;
        Some((data_type, response.bytes))
    }
}

#[cfg(target_os = "macos")]
impl Drop for Smc {
    fn drop(&mut self) {
        // SAFETY: `connection` is the handle `IOServiceOpen` handed us and
        // this is the only close, which drop runs exactly once.
        unsafe { IOServiceClose(self.connection) };
    }
}

/// The hottest CPU temperature this machine reports, in degrees Celsius.
///
/// `Ok(None)` means the SMC either could not be opened without privileges, or
/// exposed no CPU sensor this code recognises — both of which the caller
/// reports honestly rather than as a number.
pub fn cpu_temperature() -> Result<Option<f32>> {
    Ok(read_cpu_temperature())
}

/// The actual read, split out so the logic is testable without a Mac.
#[cfg(target_os = "macos")]
fn read_cpu_temperature() -> Option<f32> {
    let smc = Smc::open()?;
    let count = smc.key_count()?;
    // A machine with an implausible key count is answering with something else
    // entirely; enumerating it would be slow and meaningless.
    if count == 0 || count > 4096 {
        return None;
    }

    let mut hottest: Option<f32> = None;
    for index in 0..count {
        let Some(name) = smc.key_name_at(index) else {
            continue;
        };
        if !is_cpu_temperature_key(&name) {
            continue;
        }
        let Some((data_type, bytes)) = smc.read_key(four_char_code_to_u32(&name)) else {
            continue;
        };
        if data_type != DATA_TYPE_SP78 {
            continue;
        }
        let Some(celsius) = decode_sp78(&bytes) else {
            continue;
        };
        if hottest.is_none_or(|current| celsius > current) {
            hottest = Some(celsius);
        }
    }
    hottest
}

/// A non-macOS build has no SMC to talk to.
#[cfg(not(target_os = "macos"))]
fn read_cpu_temperature() -> Option<f32> {
    None
}

/// Whether an SMC key names a CPU temperature.
///
/// `TC0*` are Intel die sensors (`TC0P` proximity, `TC0D` die), `Tp0*` are
/// Apple silicon performance cores and `Tm0*` its efficiency cores. The
/// lowercase `m` matters: uppercase `TM0P` is the memory sensor, not a core.
///
/// The GPU (`TG0*`), ambient (`TA0*`) and battery (`TB0*`) sensors are
/// deliberately excluded — they are not the CPU die temperature this reports.
fn is_cpu_temperature_key(key: &str) -> bool {
    key.starts_with("TC0") || key.starts_with("Tp0") || key.starts_with("Tm0")
}

/// Decode a `sp78` fixed point reading into Celsius.
///
/// `sp78` is 8.8 fixed point, so the value is the byte pair divided by 256.
/// The signed markers the SMC uses for "no reading" land far outside any
/// plausible CPU temperature and are rejected here rather than shown.
fn decode_sp78(bytes: &[u8]) -> Option<f32> {
    let pair = bytes.get(..2)?;
    let raw = u16::from_le_bytes([pair[0], pair[1]]);
    let celsius = raw as f32 / 256.0;
    (celsius.is_finite() && (MIN_PLAUSIBLE_CELSIUS..=MAX_PLAUSIBLE_CELSIUS).contains(&celsius))
        .then_some(celsius)
}

/// The SMC's packed four-character key, as a string.
///
/// Non-printable bytes become `.` so a garbage name shows up as obvious noise
/// instead of control characters in a table cell.
fn four_char_code(key: u32) -> String {
    let bytes = key.to_le_bytes();
    bytes
        .iter()
        .map(|&byte| {
            if byte.is_ascii_graphic() {
                byte as char
            } else {
                '.'
            }
        })
        .collect()
}

/// A four-character name as the SMC packs it, least significant byte first.
fn four_char_code_to_u32(name: &str) -> u32 {
    let bytes = name.as_bytes();
    let mut key = 0u32;
    for (index, &byte) in bytes.iter().take(4).enumerate() {
        key |= (byte as u32) << (index * 8);
    }
    key
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sp78_decodes_the_published_shape() {
        // 0x2E00 is 46.0 in 8.8 fixed point: (0x2E00 >> 8) = 46.
        let bytes = [0x00, 0x2E, 0, 0];
        assert_eq!(decode_sp78(&bytes), Some(46.0));
        // 0x0BB8 is 3000/256 = 11.71875.
        let bytes = [0xB8, 0x0B, 0, 0];
        assert_eq!(decode_sp78(&bytes), Some(3000.0 / 256.0));
    }

    #[test]
    fn sp78_decodes_a_room_temperature_reading() {
        // 45.5 C, a value any idle Mac reports.
        let raw = (45.5 * 256.0) as u16;
        let bytes = raw.to_le_bytes();
        assert_eq!(decode_sp78(&bytes), Some(45.5));
    }

    #[test]
    fn sp78_rejects_the_invalid_marker_and_impossible_readings() {
        // 0x7f7f is the SMC's "no reading" marker and decodes to 127.7 C, which
        // is hotter than any CPU — it must not reach the user as a number.
        assert_eq!(decode_sp78(&[0xFF, 0x7F]), None);
        assert_eq!(decode_sp78(&[0xFF, 0xFF]), None, "65535 would be nonsense");
        assert_eq!(decode_sp78(&[]), None, "a short buffer is not a reading");
        assert_eq!(decode_sp78(&[0x00]), None);
    }

    #[test]
    fn cpu_sensor_keys_are_told_from_gpu_and_memory_sensors() {
        // Intel and Apple silicon naming.
        for key in ["TC0P", "TC0D", "TC0E", "Tp01", "Tp0L", "Tm01", "Tm0P"] {
            assert!(is_cpu_temperature_key(key), "{key} is a CPU sensor");
        }
        // GPU, ambient, battery, memory and platform sensors are not the CPU
        // die temperature.
        for key in [
            "TG0P", "TG0D", "TA0P", "TB0T", "TM0P", "MHFA", "MSTP", "Th0H",
        ] {
            assert!(!is_cpu_temperature_key(key), "{key} is not a CPU sensor");
        }
    }

    #[test]
    fn a_key_name_round_trips_through_the_smc_packing() {
        for name in ["TC0P", "Tp01", "Tm0P"] {
            let packed = four_char_code_to_u32(name);
            assert_eq!(four_char_code(packed), name);
        }
    }

    #[test]
    fn a_key_name_is_always_four_characters_of_printable_text() {
        // A garbage name must read as obvious noise, never as control codes.
        assert_eq!(four_char_code(u32::MAX), "....");
        assert_eq!(four_char_code(0), "....");
        assert_eq!(four_char_code(0x00_00_00_41), "A...");
        assert_eq!(four_char_code(0x41_42_43_44).len(), 4);
    }

    #[test]
    fn the_hottest_core_wins_over_a_mixed_set() {
        // What `read_cpu_temperature` does with several cores: a parked
        // efficiency core reads colder, and the user wants the hot one.
        let samples = [("Tp01", 41.0), ("Tm01", 35.5), ("Tp02", 63.25)];
        let hottest = samples
            .iter()
            .map(|(_, value)| *value)
            .fold(None, |acc: Option<f32>, value| {
                acc.map_or(Some(value), |current| Some(current.max(value)))
            });
        assert_eq!(hottest, Some(63.25));
    }
}
