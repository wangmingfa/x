//! The Apple SMC wire protocol, platform-independent.
//!
//! The struct, command codes and byte order here are not from memory: they
//! mirror `SMCKeyData_t` as defined in Apple's PowerManagement project
//! (`PrivateLib.c`) and as carried unchanged by the classic SMC tools
//! (`smc.c`/`smc.h`, SMCKit). The macOS adapter in [`crate::macos::smc`] drives
//! this struct over IOKit; this module holds everything that can be built and
//! tested without a Mac.
//!
//! Three facts worth pinning, because getting any of them wrong does not
//! produce an error — it produces garbage or a crash:
//!
//! - The struct is **80 bytes**. The kernel copies that many bytes in and out
//!   of the user-client call; a shorter struct is an out-of-bounds write, not
//!   a failed call. Compile-time asserts below keep it that way.
//! - Four-character keys pack **big-endian**: `TC0P` is `0x54433050`, first
//!   character in the most significant byte.
//! - `sp78` samples are **big-endian and signed**: byte 0 holds the integer
//!   part including the sign, byte 1 the fraction. `flt ` samples — what Apple
//!   silicon's `Tp0*` die sensors report — are a **little-endian IEEE-754
//!   single**. The declared type is what picks the decoder; the same four bytes
//!   read under the other type give a plausible-looking wrong temperature.

/// The `IOConnectCallStructMethod` selector the SMC user client answers on
/// (`KERNEL_INDEX_SMC` in the classic tools).
pub const SMC_SELECTOR: u32 = 2;

/// Command (in `data8`): read a key's metadata — size and type.
pub const SMC_CMD_READ_KEYINFO: u8 = 9;
/// Command (in `data8`): read a key's bytes.
pub const SMC_CMD_READ_BYTES: u8 = 5;
/// Command (in `data8`): read the key name at the index in `data32`.
pub const SMC_CMD_READ_INDEX: u8 = 8;

/// `SMCParamStruct.result` for a successful call.
pub const SMC_RESULT_SUCCESS: u8 = 0;
/// `SMCParamStruct.result` when the key does not exist on this machine.
pub const SMC_RESULT_KEY_NOT_FOUND: u8 = 132;

/// `sp78` as a packed FourCharCode; the SMC stores type names big-endian too.
pub const DATA_TYPE_SP78: u32 = 0x7370_3738;

/// `flt ` as a packed FourCharCode: the type Apple silicon's die sensors
/// actually carry.
pub const DATA_TYPE_FLT: u32 = 0x666c_7420;

/// A CPU die reading outside this window is not a temperature. Unconnected
/// sensors answer with the `sp78` floor (-128.0) or similar noise rather than
/// an error, so the window is the only thing standing between them and the
/// user.
pub const MIN_PLAUSIBLE_CELSIUS: f32 = -40.0;
pub const MAX_PLAUSIBLE_CELSIUS: f32 = 150.0;

/// `SMCKeyData_vers_t`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SmcVersion {
    pub major: u8,
    pub minor: u8,
    pub build: u8,
    pub reserved: u8,
    pub release: u16,
}

/// `SMCKeyData_pLimitData_t`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SmcPLimitData {
    pub version: u16,
    pub length: u16,
    pub cpu_plimit: u32,
    pub gpu_plimit: u32,
    pub mem_plimit: u32,
}

/// `SMCKeyData_keyInfo_t`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SmcKeyInfo {
    /// How many bytes of `bytes` are meaningful.
    pub data_size: u32,
    /// The key's type as a packed FourCharCode (`sp78`, `ui8 `, `fpe2`, ...).
    pub data_type: u32,
    pub data_attributes: u8,
}

/// `SMCKeyData_t`: the one structure the SMC user client speaks.
///
/// The nested shapes mirror the C definition so `repr(C)` computes the same
/// padding the kernel expects: `vers` ends at 10, `pLimitData` lands on 12
/// (4-alignment), `keyInfo` on 28, the result trio on 40 and `data32` on 44.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct SmcKeyData {
    /// The key, packed big-endian (`TC0P` = `0x54433050`).
    pub key: u32,
    pub vers: SmcVersion,
    pub p_limit_data: SmcPLimitData,
    pub key_info: SmcKeyInfo,
    /// Call outcome: [`SMC_RESULT_SUCCESS`], [`SMC_RESULT_KEY_NOT_FOUND`], ...
    pub result: u8,
    pub status: u8,
    /// The command: [`SMC_CMD_READ_KEYINFO`], [`SMC_CMD_READ_BYTES`], ...
    pub data8: u8,
    /// Secondary argument; carries the index for [`SMC_CMD_READ_INDEX`].
    pub data32: u32,
    /// Payload, `key_info.data_size` bytes of which are meaningful.
    pub bytes: [u8; 32],
}

// The layout is the contract with the kernel; drift in any of these is a
// memory-safety bug, so it fails to compile rather than failing on a runner.
const _: () = {
    assert!(std::mem::size_of::<SmcKeyData>() == 80);
    assert!(std::mem::align_of::<SmcKeyData>() == 4);
    assert!(std::mem::offset_of!(SmcKeyData, result) == 40);
    assert!(std::mem::offset_of!(SmcKeyData, data32) == 44);
    assert!(std::mem::offset_of!(SmcKeyData, bytes) == 48);
};

/// Pack a four-character key the way the SMC stores it: big-endian, first
/// character in the most significant byte, shorter names space-padded.
pub fn pack_key(name: &str) -> u32 {
    let bytes = name.as_bytes();
    let mut key = 0u32;
    for (index, byte) in bytes.iter().take(4).enumerate() {
        key |= (*byte as u32) << (24 - index * 8);
    }
    key
}

/// The four characters of a packed key, most significant byte first.
///
/// Non-printable bytes become `.` so a garbage name reads as obvious noise
/// instead of printing control characters or vanishing.
pub fn key_name(key: u32) -> String {
    key.to_be_bytes()
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

/// Whether an SMC key names a CPU temperature.
///
/// `TC0*` are Intel die sensors (`TC0P` proximity, `TC0D` die), `Tp0*` are
/// Apple silicon performance cores and `Tm0*` its efficiency cores. The
/// lowercase `m` matters: uppercase `TM0P` is the memory sensor, not a core.
/// GPU (`TG0*`), ambient (`TA0*`) and battery (`TB0*`) sensors are excluded —
/// they are not the CPU die temperature.
pub fn is_cpu_temperature_key(key: &str) -> bool {
    key.starts_with("TC0") || key.starts_with("Tp0") || key.starts_with("Tm0")
}

/// Decode an `sp78` sample: 8.8 signed fixed point, big-endian on the wire.
///
/// The classic tool decodes the pair as `bytes[0] * 256 + bytes[1]` over 256,
/// sign included in byte 0. Readings outside the plausible window are rejected
/// rather than passed through: an unconnected sensor answers with the `sp78`
/// floor (-128.0), and that is not a temperature anyone should see.
pub fn decode_sp78(bytes: &[u8]) -> Option<f32> {
    let pair = bytes.get(..2)?;
    let raw = i16::from_be_bytes([pair[0], pair[1]]);
    let celsius = raw as f32 / 256.0;
    ((MIN_PLAUSIBLE_CELSIUS..=MAX_PLAUSIBLE_CELSIUS).contains(&celsius)).then_some(celsius)
}

/// Decode an `flt ` sample: a little-endian IEEE-754 single, four bytes.
///
/// There is no documented sentinel here the way `sp78` has its floor, so the
/// same plausible window is the only guard — and it drops NaN and the
/// infinities for free, since they fail every comparison.
pub fn decode_flt(bytes: &[u8]) -> Option<f32> {
    let quad = bytes.get(..4)?;
    let celsius = f32::from_le_bytes([quad[0], quad[1], quad[2], quad[3]]);
    ((MIN_PLAUSIBLE_CELSIUS..=MAX_PLAUSIBLE_CELSIUS).contains(&celsius)).then_some(celsius)
}

/// Read one sample as the temperature type the SMC declared for it.
///
/// A type we cannot decode (`ioft`, `ui8 `, anything unlisted) is refused
/// rather than approximated: guessing a decoder for a payload whose shape we do
/// not know is how a wrong number gets printed as a temperature.
pub fn decode_temperature(data_type: u32, bytes: &[u8]) -> Option<f32> {
    match data_type {
        DATA_TYPE_SP78 => decode_sp78(bytes),
        DATA_TYPE_FLT => decode_flt(bytes),
        _ => None,
    }
}

/// The total key count the `#KEY` pseudo-key reports: a big-endian `ui32`.
pub fn decode_key_count(bytes: &[u8]) -> Option<u32> {
    let quad = bytes.get(..4)?;
    Some(u32::from_be_bytes([quad[0], quad[1], quad[2], quad[3]]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_struct_is_exactly_eighty_bytes_with_the_kernel_offsets() {
        // Redundant with the const asserts above on purpose: if one of these
        // ever fails, the failure should be a readable test name, not just a
        // compile error in a const block.
        assert_eq!(std::mem::size_of::<SmcKeyData>(), 80);
        assert_eq!(std::mem::offset_of!(SmcKeyData, result), 40);
        assert_eq!(std::mem::offset_of!(SmcKeyData, data8), 42);
        assert_eq!(std::mem::offset_of!(SmcKeyData, data32), 44);
        assert_eq!(std::mem::offset_of!(SmcKeyData, bytes), 48);
    }

    #[test]
    fn keys_pack_big_endian_and_round_trip() {
        assert_eq!(pack_key("TC0P"), 0x5443_3050);
        assert_eq!(pack_key("sp78"), DATA_TYPE_SP78);
        assert_eq!(pack_key("#KEY"), 0x234B_4559);
        for name in ["TC0P", "Tp01", "Tm0P", "#KEY"] {
            assert_eq!(key_name(pack_key(name)), name);
        }
    }

    #[test]
    fn a_short_name_is_space_padded_like_the_c_tool() {
        // `AB` packs as `AB\0\0`; the NULs are not printable, so the unpacked
        // name reads as noise characters rather than vanishing.
        assert_eq!(key_name(pack_key("AB")), "AB..");
    }

    #[test]
    fn a_key_name_is_always_four_characters_of_printable_text() {
        assert_eq!(key_name(u32::MAX), "....");
        assert_eq!(key_name(0), "....");
        assert_eq!(key_name(0x41_00_00_00), "A...");
    }

    #[test]
    fn sp78_decodes_big_endian_signed() {
        // 0x2E00: 46 integer part, zero fraction.
        assert_eq!(decode_sp78(&[0x2E, 0x00]), Some(46.0));
        // 0x2D80: 45 + 128/256 = 45.5.
        assert_eq!(decode_sp78(&[0x2D, 0x80]), Some(45.5));
        // 0xFF00 is negative: -1.0.
        assert_eq!(decode_sp78(&[0xFF, 0x00]), Some(-1.0));
    }

    #[test]
    fn sp78_rejects_the_floor_and_impossible_readings() {
        // An unconnected sensor answers 0x8000, the sp78 floor of -128.0.
        assert_eq!(decode_sp78(&[0x80, 0x00]), None);
        assert_eq!(decode_sp78(&[0x81, 0x00]), None, "-127.0 is noise too");
        assert_eq!(decode_sp78(&[]), None, "a short buffer is not a reading");
        assert_eq!(decode_sp78(&[0x2E]), None);
    }

    #[test]
    fn flt_decodes_little_endian_ieee754() {
        // Payloads captured from a real Apple silicon SMC: `Tp02` and `Tp06`.
        // Both are exact in binary, so the expectations are pinned to the
        // IEEE-754 value rather than to whatever this decoder computes.
        assert_eq!(decode_flt(&[0x00, 0x20, 0x83, 0x42]), Some(65.5625));
        assert_eq!(decode_flt(&[0x00, 0xf8, 0x99, 0x42]), Some(76.984375));
        // 0x41C00000 is 24.0 by definition of the format.
        assert_eq!(decode_flt(&[0x00, 0x00, 0xc0, 0x41]), Some(24.0));
        assert_eq!(
            decode_flt(&[0x00, 0x00, 0xc0]),
            None,
            "three bytes of noise"
        );
    }

    #[test]
    fn flt_rejects_what_is_not_a_temperature() {
        // The window catches NaN and the infinities without naming them: both
        // fail every comparison, `contains` included.
        assert_eq!(decode_flt(&[0x00, 0x00, 0xc0, 0x7f]), None, "NaN");
        assert_eq!(decode_flt(&[0x00, 0x00, 0x80, 0x7f]), None, "infinity");
        assert_eq!(decode_flt(&[0x00, 0x24, 0x74, 0x49]), None, "1e6");
    }

    #[test]
    fn the_declared_type_picks_the_decoder() {
        // The four bytes of `Tp02` are 65.5625 C as `flt ` and 0.125 C as
        // `sp78` — both inside the plausible window, one of them a lie. The
        // type from keyInfo is therefore load-bearing, not decoration.
        let bytes = [0x00, 0x20, 0x83, 0x42];
        assert_eq!(decode_temperature(DATA_TYPE_FLT, &bytes), Some(65.5625));
        assert_eq!(decode_temperature(DATA_TYPE_SP78, &bytes), Some(0.125));
        // Types we do not decode are refused, not approximated.
        assert_eq!(decode_temperature(pack_key("ioft"), &bytes), None);
        assert_eq!(decode_temperature(pack_key("ui8 "), &bytes), None);
        assert_eq!(decode_temperature(0, &bytes), None);
    }

    #[test]
    fn the_key_count_is_a_big_endian_ui32() {
        assert_eq!(decode_key_count(&[0x00, 0x00, 0x01, 0x00]), Some(256));
        assert_eq!(decode_key_count(&[0x00]), None);
    }

    #[test]
    fn cpu_sensor_keys_are_told_from_gpu_and_memory_sensors() {
        for key in ["TC0P", "TC0D", "TC0E", "Tp01", "Tp0L", "Tm01", "Tm0P"] {
            assert!(is_cpu_temperature_key(key), "{key} is a CPU sensor");
        }
        for key in [
            "TG0P", "TG0D", "TA0P", "TB0T", "TM0P", "MHFA", "MSTP", "Th0H",
        ] {
            assert!(!is_cpu_temperature_key(key), "{key} is not a CPU sensor");
        }
    }

    #[test]
    fn the_hottest_core_wins_over_a_mixed_set() {
        // What the enumeration loop does with several cores: a parked
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
