//! The Apple SMC wire protocol, platform-independent.
//!
//! The struct, command codes and byte order here are not from memory: they
//! mirror `SMCKeyData_t` as defined in Apple's PowerManagement project
//! (`PrivateLib.c`) and as carried unchanged by the classic SMC tools
//! (`smc.c`/`smc.h`, SMCKit). The macOS adapter in [`crate::macos::smc`] drives
//! this struct over IOKit; this module holds everything that can be built and
//! tested without a Mac.
//!
//! The split is at the byte exchange, not at the decode: [`RoundTrip`] is one
//! 80-byte call, [`SmcClient`] is everything the protocol requires on top of it
//! (count the keys, enumerate them, filter, read, type-dispatch, aggregate).
//! The macOS side therefore supplies only IOKit plumbing, and the loop that can
//! get a protocol fact wrong — and does, in the wild — is replayed against
//! transcripts recorded from real hardware on every platform CI builds.
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

use std::collections::BTreeMap;

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

/// How many payload bytes a key can carry (`SMCKeyData_t::bytes`). A declared
/// `data_size` above this is not a size the struct can hold, so it is treated as
/// a broken answer rather than a read.
pub const MAX_KEY_BYTES: usize = 32;

/// Above this, the answer to `#KEY` is not a key count worth enumerating. Real
/// machines measured 2109 keys; a machine claiming tens of thousands is
/// answering with something other than an SMC, and enumerating it would be slow
/// and meaningless.
pub const MAX_ENUMERATED_KEYS: u32 = 4096;

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
    pub bytes: [u8; MAX_KEY_BYTES],
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
/// The families are what a real machine's table says, not what the naming
/// convention suggests. On a Mac16,9 the 2109 keys include `Tp0*` (45), `Tp2*`
/// (31), `Ts0*` (30), `Tp1*` (14) and `Tpx*` (12) — all typed `flt `, all
/// answering inside the temperature range, and the `Tp1*`/`Tpx*` clusters
/// reading *hotter* than any `Tp0*` key (82.1 ℃ against 78.3 ℃ in the same
/// read), which is why restricting the set to `Tp0*` under-reported the die.
///
/// What stays out, measured from that same table: `TG0*`/`TA0*`/`TB0*` (GPU,
/// ambient, battery), the uppercase lookalikes `TPD0`/`TPMp`/`TPSD`/`TS0p`/
/// `TSV*`/`TMVR`/`TH0x`, and the partial-cluster forms `TCDX`/`TCMb`/`TCMz` —
/// `TC0` is the Intel die prefix, `TCM`/`TCD` are not. `Tsx0`/`Tsx1` are the
/// one family in the grey area: they read 43–54 ℃ like their `Ts0*` neighbours
/// but were not part of the decision to widen, so they are not read.
///
/// Case matters throughout: the lowercase `m` in `Tm0` is a core, uppercase
/// `TM0P` is the memory sensor.
pub fn is_cpu_temperature_key(key: &str) -> bool {
    key.starts_with("TC0")
        || key.starts_with("Tp")
        || key.starts_with("Tm0")
        || key.starts_with("Ts0")
}

/// The cluster a packed key belongs to: its first three characters.
///
/// A placeholder window is a property of one cluster, not of the machine: in a
/// measured read all four `Tp*` clusters were on placeholder sets (5, 4, 5 and 1
/// distinct values) while `Ts0*` answered 20 distinct real temperatures. Each
/// cluster is therefore judged on its own spread, by [`hottest_when_differentiated`].
///
/// The split takes the leading three bytes of the packed value. For printable
/// names that is the leading three characters; where a name has a `.`, the
/// packed split is the honest one, because several different bytes print as the
/// same dot.
fn sensor_cluster(key: u32) -> u32 {
    key >> 8
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

/// The hottest sample of one cluster, but only once that cluster has
/// differentiated itself.
///
/// Apple silicon answers these keys in whole-cluster states rather than key by
/// key: a measured window several seconds long had all 45 `Tp0*` keys at exactly
/// `40.0`, and another had them on {−4.0, 0.0, 2.5, 4.0, 5.2}. Both sit inside
/// the plausible range, so neither the type dispatch nor that range can catch
/// them — the collapse of the *set* is the only signal available. Real readings
/// of the same 45 keys measured 44–45 distinct values, so asking for half of
/// them to differ leaves a wide margin; half-switched windows (measured 7–16
/// distinct) are refused along with them, because a hot core appearing among
/// still-undifferentiated neighbours is the table partway through a switch, not
/// a reading to print.
///
/// The margin per cluster, over 50 reads of the real table: `Tp0*` needs 23 and
/// measures 44–45 when live, 5 when placeholdered; `Tp1*` needs 7, measures 14
/// or 4–5; `Tp2*` needs 16, measures 31 or 4–6; `Tpx*` needs 6, measures 12 or
/// 1–2. The narrowest is `Ts0*`, which needs 15 and measures 19–20 in every
/// sample — it quantizes across its 30 keys even when answering for real, so its
/// margin is 4–5 rather than double the threshold.
///
/// Exact float equality is the point: the placeholder repeats byte-identically
/// across keys, and that repetition is what is being detected.
///
/// One sample is never censored — a single key cannot show a collapse, and
/// refusing it would silence machines that only expose one die sensor.
pub fn hottest_when_differentiated(samples: &[f32]) -> Option<f32> {
    let mut distinct: Vec<f32> = Vec::new();
    let mut hottest: Option<f32> = None;
    for &sample in samples {
        hottest = Some(hottest.map_or(sample, |current| current.max(sample)));
        if !distinct.contains(&sample) {
            distinct.push(sample);
        }
    }
    if distinct.len() < samples.len().div_ceil(2) {
        return None;
    }
    hottest
}

/// The total key count the `#KEY` pseudo-key reports: a big-endian `ui32`.
pub fn decode_key_count(bytes: &[u8]) -> Option<u32> {
    let quad = bytes.get(..4)?;
    Some(u32::from_be_bytes([quad[0], quad[1], quad[2], quad[3]]))
}

/// One 80-byte exchange with an SMC, whatever it is driven over: the IOKit user
/// client on a Mac, a transcript recorded from real hardware in a test.
///
/// `None` means the transport itself did not answer — no service, a kernel
/// return that is not success. A struct that came back with a failing `result`
/// byte is returned as-is: reading that is [`SmcClient::call`]'s job, so both
/// kinds of failure are checked in one place and a replay can produce either.
pub trait RoundTrip {
    fn round_trip(&self, input: &SmcKeyData) -> Option<SmcKeyData>;
}

/// The SMC reads that together answer "how hot is this CPU", driven over any
/// [`RoundTrip`].
///
/// This is the part worth testing on a machine without an SMC: every step is a
/// protocol decision (which command, which size, which keys count as the die,
/// what to do when one key refuses) and each one has a way to fail that shows up
/// as a wrong number rather than an error.
pub struct SmcClient<R: RoundTrip> {
    transport: R,
}

impl<R: RoundTrip> SmcClient<R> {
    pub fn new(transport: R) -> Self {
        Self { transport }
    }

    /// One command, accepted only when the SMC's own `result` byte agrees.
    fn call(&self, input: &SmcKeyData) -> Option<SmcKeyData> {
        let output = self.transport.round_trip(input)?;
        if output.result != SMC_RESULT_SUCCESS {
            return None;
        }
        Some(output)
    }

    /// A key's declared type and payload. Keyinfo comes first because the size
    /// it reports is the size the byte read asks for; a size of zero or one
    /// larger than the payload buffer is a broken answer, not a read.
    pub fn read_key(&self, key: u32) -> Option<(u32, [u8; MAX_KEY_BYTES])> {
        let info = self.call(&SmcKeyData {
            key,
            data8: SMC_CMD_READ_KEYINFO,
            ..SmcKeyData::default()
        })?;
        let size = info.key_info.data_size;
        if size == 0 || size as usize > MAX_KEY_BYTES {
            return None;
        }
        let value = self.call(&SmcKeyData {
            key,
            data8: SMC_CMD_READ_BYTES,
            key_info: SmcKeyInfo {
                data_size: size,
                ..SmcKeyInfo::default()
            },
            ..SmcKeyData::default()
        })?;
        Some((info.key_info.data_type, value.bytes))
    }

    /// Total number of keys the SMC holds, via the `#KEY` pseudo-key.
    pub fn key_count(&self) -> Option<u32> {
        let (_data_type, bytes) = self.read_key(pack_key("#KEY"))?;
        decode_key_count(&bytes)
    }

    /// The key at `index`, as its packed value and its printable name.
    ///
    /// The value is what gets read back. Repacking the printed name would be
    /// lossy for a key carrying a non-printable byte — real tables have one,
    /// `LS! ` — and a filter that reads the name must not quietly change which
    /// value is then asked for.
    pub fn key_at(&self, index: u32) -> Option<(u32, String)> {
        let out = self.call(&SmcKeyData {
            data8: SMC_CMD_READ_INDEX,
            data32: index,
            ..SmcKeyData::default()
        })?;
        Some((out.key, key_name(out.key)))
    }

    /// The hottest CPU temperature, or `None` when the machine did not say.
    ///
    /// A key that refuses is skipped, not fatal: one sensor out of the table may
    /// be absent, and the rest still answer.
    pub fn cpu_temperature(&self) -> Option<f32> {
        let count = self.key_count()?;
        if count == 0 || count > MAX_ENUMERATED_KEYS {
            return None;
        }

        let mut clusters: BTreeMap<u32, Vec<f32>> = BTreeMap::new();
        for index in 0..count {
            let Some((key, name)) = self.key_at(index) else {
                continue;
            };
            if !is_cpu_temperature_key(&name) {
                continue;
            }
            let Some((data_type, bytes)) = self.read_key(key) else {
                continue;
            };
            if let Some(celsius) = decode_temperature(data_type, &bytes) {
                clusters
                    .entry(sensor_cluster(key))
                    .or_default()
                    .push(celsius);
            }
        }
        // Each cluster is judged whole and on its own spread. Folding the max
        // while reading hides the collapse, because a single hot core looks
        // plausible on its own; pooling every cluster into one decision needs a
        // threshold over all 132 keys, and measured real reads cleared that
        // (69 distinct against a required 66) by three values instead of the
        // several-fold margin each cluster keeps against its own half.
        let mut hottest: Option<f32> = None;
        for samples in clusters.values() {
            if let Some(celsius) = hottest_when_differentiated(samples) {
                hottest = Some(hottest.map_or(celsius, |current| current.max(celsius)));
            }
        }
        hottest
    }
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
        for key in [
            "TC0P", "TC0D", "TC0E", "Tp01", "Tp0L", "Tm01", "Tm0P", "Tp1i", "Tp20", "Tpx0", "Ts00",
            "Ts0I",
        ] {
            assert!(is_cpu_temperature_key(key), "{key} is a CPU sensor");
        }
        for key in [
            "TG0P", "TG0D", "TA0P", "TB0T", "TM0P", "MHFA", "MSTP", "Th0H", "TS0p", "Tsx0", "TPD0",
            "TPMp", "TMVR", "TH0x", "TCDX", "TCMb",
        ] {
            assert!(!is_cpu_temperature_key(key), "{key} is not a CPU sensor");
        }
    }

    #[test]
    fn keys_group_into_their_sensor_cluster() {
        // The partition the differentiation guard is applied over: same first
        // three characters, same cluster; and `Tp1*`/`Tpx*`/`Ts0*` are separate
        // from `Tp0*`, which is what keeps a silent cluster from being judged by
        // a talking one.
        for (a, b) in [("Tp01", "Tp0z"), ("Tp1i", "Tp1o"), ("Ts00", "Ts0I")] {
            assert_eq!(sensor_cluster(pack_key(a)), sensor_cluster(pack_key(b)));
        }
        let clusters = ["Tp0L", "Tp1L", "Tp2L", "TpxL", "Ts0L"];
        for (index, key) in clusters.iter().enumerate() {
            for other in &clusters[index + 1..] {
                assert_ne!(
                    sensor_cluster(pack_key(key)),
                    sensor_cluster(pack_key(other)),
                    "{key} and {other} are different clusters"
                );
            }
        }
    }

    #[test]
    fn the_hottest_core_wins_over_a_mixed_set() {
        // What the read loop gets with several cores: a parked efficiency core
        // reads colder, and the user wants the hot one.
        let samples = [41.0, 35.5, 63.25];
        assert_eq!(hottest_when_differentiated(&samples), Some(63.25));
    }

    #[test]
    fn a_group_that_has_not_differentiated_is_not_a_temperature() {
        // Measured on a real machine: for seconds at a time all 45 `Tp0*` keys
        // answer the same 40.0. It is inside the plausible window, so only the
        // collapse of the set gives it away.
        let flat = [40.0; 45];
        assert_eq!(
            hottest_when_differentiated(&flat),
            None,
            "45 keys on one value is the sensor table not yet saying"
        );

        // The other measured shape: the whole set lands on five quantized
        // values, repeating every four keys.
        let pattern = [-4.0, 0.0, 2.5, 4.0, 5.2];
        let quantized: Vec<f32> = (0..45).map(|i| pattern[i % pattern.len()]).collect();
        assert_eq!(hottest_when_differentiated(&quantized), None);
        assert!(
            quantized
                .iter()
                .all(|&t| (MIN_PLAUSIBLE_CELSIUS..=MAX_PLAUSIBLE_CELSIUS).contains(&t)),
            "each placeholder passes the plausibility window on its own"
        );
    }

    #[test]
    fn a_real_reading_of_the_same_keys_passes() {
        // The measured value set of one real read (16 distinct across the keys,
        // spanning more than 25 degrees).
        let real = [
            50.70, 58.70, 67.64, 53.38, 59.88, 76.08, 49.53, 57.53, 64.44, 52.44, 58.94, 73.20,
            48.54, 56.54, 65.45, 50.15,
        ];
        assert_eq!(hottest_when_differentiated(&real), Some(76.08));
    }

    #[test]
    fn a_window_partway_through_a_switch_is_refused_too() {
        // Measured transition row: 43 keys still on the placeholder set while
        // two cores already answer real temperatures. The hot core looks
        // perfectly reasonable in isolation, which is exactly why the group is
        // judged together.
        let mut mixed: Vec<f32> = (0..43).map(|i| [-4.0, 0.0, 2.5, 4.0, 5.2][i % 5]).collect();
        mixed.extend_from_slice(&[66.609375, 44.978]);
        assert_eq!(hottest_when_differentiated(&mixed), None);
        assert_eq!(
            mixed.iter().copied().fold(f32::MIN, f32::max),
            66.609375,
            "the hot core was there; refusing it is the point of the rule"
        );
    }

    #[test]
    fn a_machine_with_one_sensor_is_never_censored() {
        // One key cannot show a collapse, so the rule must not silence Intel
        // machines that expose a single die sensor.
        assert_eq!(hottest_when_differentiated(&[37.0]), Some(37.0));
        assert_eq!(hottest_when_differentiated(&[40.0, 40.0]), Some(40.0));
        assert_eq!(hottest_when_differentiated(&[]), None, "no answer at all");
    }

    // Replay: the read loop driven by transcripts recorded from a real AppleSMC.
    // On a Linux or Windows CI job this is the only way the protocol code runs
    // at all, and the IOKit transport is a one-call shim these tests never touch.

    use std::collections::HashMap;

    /// A parsed transcript: enough for a transport to answer as the machine did.
    #[derive(Clone, Default)]
    struct Transcript {
        count: Option<u32>,
        keys: Vec<u32>,
        infos: HashMap<u32, (u32, u32)>,
        payloads: HashMap<u32, [u8; MAX_KEY_BYTES]>,
    }

    /// `KEY Tp01` and `KEY 0x4c532120` name the same record. The recorder falls
    /// back to hex when a key carries a byte that is not printable, because only
    /// the value survives that.
    fn parse_key(field: &str) -> u32 {
        match field.strip_prefix("0x") {
            Some(hex) => u32::from_str_radix(hex, 16).expect("`KEY 0x…` is eight hex digits"),
            None => pack_key(field),
        }
    }

    fn parse_hex(hex: &str) -> Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("payload is hex"))
            .collect()
    }

    fn parse_transcript(text: &str) -> Transcript {
        let mut transcript = Transcript::default();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut fields = line.split_whitespace();
            match fields.next() {
                Some("COUNT") => {
                    let value = fields.next().expect("COUNT carries a number");
                    transcript.count = Some(value.parse().expect("COUNT is a key count"));
                    assert_eq!(fields.next(), None, "COUNT takes one field");
                }
                Some("KEY") => {
                    let name = fields.next().expect("KEY carries a name");
                    transcript.keys.push(parse_key(name));
                    assert_eq!(fields.next(), None, "KEY takes one field");
                }
                Some("DATA") => {
                    let name = fields.next().expect("DATA carries a name");
                    let key = parse_key(name);
                    assert_eq!(
                        key_name(key),
                        name,
                        "`{name}` must repack to the key it names"
                    );
                    let data_type =
                        u32::from_str_radix(fields.next().expect("DATA carries a type"), 16)
                            .expect("the type is hex");
                    let data_size: u32 = fields
                        .next()
                        .expect("DATA carries a size")
                        .parse()
                        .expect("the size is a number");
                    let hex = fields.next().unwrap_or_default();
                    let bytes = parse_hex(hex);
                    assert!(
                        bytes.len() as u32 <= data_size && bytes.len() <= MAX_KEY_BYTES,
                        "payload is not longer than the declared size or the buffer"
                    );
                    let mut padded = [0u8; MAX_KEY_BYTES];
                    padded[..bytes.len()].copy_from_slice(&bytes);
                    transcript.infos.insert(key, (data_type, data_size));
                    transcript.payloads.insert(key, padded);
                    assert_eq!(fields.next(), None, "DATA takes four fields");
                }
                other => panic!("unrecognised record in the transcript: {other:?}"),
            }
        }
        transcript
    }

    impl Transcript {
        /// This enumeration and metadata, answering with another capture's
        /// payloads. Both files come from one machine minutes apart, where the
        /// recorded key table and every declared type and size were identical —
        /// only the values moved.
        fn with_values_from(&self, values: &Transcript) -> Transcript {
            for key in values.payloads.keys() {
                assert!(
                    self.infos.contains_key(key),
                    "the second capture read a key the first did not list"
                );
            }
            Transcript {
                count: self.count,
                keys: self.keys.clone(),
                infos: self.infos.clone(),
                payloads: values.payloads.clone(),
            }
        }
    }

    /// A [`RoundTrip`] that answers from a transcript instead of a kernel.
    struct Replay {
        transcript: Transcript,
    }

    impl RoundTrip for Replay {
        fn round_trip(&self, input: &SmcKeyData) -> Option<SmcKeyData> {
            // A key the table does not hold is answered the way a real SMC
            // answers it — a reply that arrived carrying a failing `result` —
            // so the two kinds of failure stay distinguishable in the loop.
            let mut out = SmcKeyData {
                key: input.key,
                result: SMC_RESULT_KEY_NOT_FOUND,
                ..SmcKeyData::default()
            };
            match input.data8 {
                SMC_CMD_READ_INDEX => {
                    if let Some(&key) = self.transcript.keys.get(input.data32 as usize) {
                        out.key = key;
                        out.result = SMC_RESULT_SUCCESS;
                    }
                }
                SMC_CMD_READ_KEYINFO => {
                    if let Some(&(data_type, data_size)) = self.transcript.infos.get(&input.key) {
                        out.key_info = SmcKeyInfo {
                            data_size,
                            data_type,
                            data_attributes: 0,
                        };
                        out.result = SMC_RESULT_SUCCESS;
                    }
                }
                SMC_CMD_READ_BYTES => {
                    if let Some(bytes) = self.transcript.payloads.get(&input.key) {
                        out.bytes = *bytes;
                        out.result = SMC_RESULT_SUCCESS;
                    }
                }
                // A command the recorder never sent: the transport itself is not
                // answering, which is the other failure the loop has to survive.
                _ => return None,
            }
            Some(out)
        }
    }

    /// 2109 keys and 132 differentiated readings in five clusters, from a
    /// Mac16,9 on macOS 15.6, recorded by a standalone C probe over IOKit.
    const DIFFERENTIATED: &str =
        include_str!("../../tests/fixtures/smc/mac16-9-2026-10-09-differentiated.txt");

    /// The same machine with every `Tp*` cluster on placeholder values and only
    /// the heat-pipe cluster answering temperatures.
    const DIE_CLUSTERS_SILENT: &str =
        include_str!("../../tests/fixtures/smc/mac16-9-2026-10-09-die-clusters-silent.txt");

    /// The same machine inside one of the undifferentiated windows.
    const COLLAPSED: &str =
        include_str!("../../tests/fixtures/smc/mac16-9-2026-10-09-collapsed-window.txt");

    fn client(transcript: Transcript) -> SmcClient<Replay> {
        SmcClient::new(Replay { transcript })
    }

    /// How many different values a cluster answered. The tests use it to make
    /// the shape they assert readable in the failure message.
    fn distinct_values(samples: &[f32]) -> usize {
        let mut unique: Vec<f32> = Vec::new();
        for &value in samples {
            if !unique.contains(&value) {
                unique.push(value);
            }
        }
        unique.len()
    }

    /// The decoded samples of one cluster, measured out of a transcript. A test
    /// that says "this cluster collapsed" should read it from the recorded bytes
    /// rather than restate the number counted by hand.
    fn cluster_samples(transcript: &Transcript, cluster: &str) -> Vec<f32> {
        transcript
            .payloads
            .iter()
            .filter(|(key, _)| key_name(**key).starts_with(cluster))
            .filter_map(|(key, bytes)| {
                let (data_type, _size) = transcript.infos[key];
                decode_temperature(data_type, bytes)
            })
            .collect()
    }

    #[test]
    fn the_parser_reads_every_record_the_recorder_wrote() {
        let table = parse_transcript(DIFFERENTIATED);
        assert_eq!(table.count, Some(2109), "the COUNT line in the file");
        // The self-check that lets the tests below mean what they say: a record
        // the parser did not understand would silently shrink one of these.
        assert_eq!(table.keys.len(), 2109, "one KEY line per index, no gaps");
        let data_lines = DIFFERENTIATED
            .lines()
            .filter(|line| line.starts_with("DATA"))
            .count();
        assert_eq!(
            table.payloads.len(),
            data_lines,
            "every DATA line became one"
        );
        assert_eq!(table.infos.len(), data_lines);
    }

    #[test]
    fn the_real_key_table_offers_exactly_the_cpu_clusters() {
        let table = parse_transcript(DIFFERENTIATED);
        let cpu: Vec<String> = table
            .keys
            .iter()
            .map(|&key| key_name(key))
            .filter(|name| is_cpu_temperature_key(name))
            .collect();
        assert_eq!(cpu.len(), 132, "the CPU keys this machine lists");
        let mut per_cluster: BTreeMap<String, usize> = BTreeMap::new();
        for name in &cpu {
            *per_cluster.entry(name[..3].to_string()).or_default() += 1;
        }
        // Five clusters, counted out of the recorded table. This is the shape the
        // guard is calibrated against: 45 keys needing 23 distinct values, 14
        // needing 7, and so on.
        assert_eq!(
            per_cluster,
            [
                ("Tp0".to_string(), 45),
                ("Tp1".to_string(), 14),
                ("Tp2".to_string(), 31),
                ("Tpx".to_string(), 12),
                ("Ts0".to_string(), 30),
            ]
            .into_iter()
            .collect()
        );
        // The neighbours the filter has to refuse, taken from the same table:
        // uppercase lookalikes, the partial-cluster forms, and the `Tsx*` pair
        // that sits next to `Ts0*` but was left out of the widened set. Any of
        // these leaking in would be a different number, not an error.
        for refused in ["TPD0", "TPMp", "TMVR", "TH0x", "TS0p", "Tsx0"] {
            assert!(
                table.keys.iter().any(|&key| key_name(key) == refused),
                "{refused} really is in the table"
            );
            assert!(
                !is_cpu_temperature_key(refused),
                "{refused} is not a CPU sensor"
            );
        }
        // And one key from each cluster the widening added, so a filter that
        // drifted back to `Tp0*`-only fails here rather than in the aggregate.
        for accepted in ["Tp1i", "Tp20", "Tpx0", "Ts00"] {
            assert!(
                table.keys.iter().any(|&key| key_name(key) == accepted),
                "{accepted} really is in the table"
            );
            assert!(is_cpu_temperature_key(accepted), "{accepted} is read");
        }
    }

    #[test]
    fn a_key_with_a_non_printable_byte_keeps_its_value() {
        // Index 325 of the real table is `LS! ` — a space in the fourth byte, so
        // the printable name cannot be packed back to the same key. The loop
        // reads by value for exactly that reason.
        let client = client(parse_transcript(DIFFERENTIATED));
        let (key, name) = client.key_at(325).expect("the table has an index 325");
        assert_eq!(key, 0x4c53_2120);
        assert_eq!(name, "LS!.");
        assert_ne!(pack_key(&name), key, "the printed name is lossy");
    }

    #[test]
    fn a_real_differentiated_transcript_reads_a_real_temperature() {
        // 82.109375 is `Tp1o`'s payload `0038a442` as a little-endian single: the
        // number the recorder's own decoder printed, arrived at separately from
        // this module's. It is the answer the widening bought — the hottest key
        // the old `Tp0*`-only filter could see is measured here to show the
        // difference, and it is nearly four degrees colder.
        let table = parse_transcript(DIFFERENTIATED);
        assert_eq!(
            client(table.clone()).cpu_temperature(),
            Some(82.109375),
            "the hottest of the clusters the filter accepts"
        );
        let tp0 = cluster_samples(&table, "Tp0");
        assert_eq!(tp0.len(), 45);
        assert_eq!(
            hottest_when_differentiated(&tp0),
            Some(78.265625),
            "what x reported while it only read `Tp0*`"
        );
    }

    #[test]
    fn a_real_window_where_only_the_heat_pipe_answers() {
        // The read that sets the rule: `Tp0*` 5 distinct values across 45 keys,
        // `Tp1*` 4 across 14, `Tp2*` 5 across 31, `Tpx*` 1 across 12 — every one
        // of them refused by its own spread — while `Ts0*` answers 20 distinct
        // temperatures across 30 keys. Hottest trusted reading: `Ts0I`, 53.734375.
        let table = parse_transcript(DIE_CLUSTERS_SILENT);
        assert_eq!(
            client(table.clone()).cpu_temperature(),
            Some(53.734375),
            "the die is silent in this read and the heat pipe is not"
        );
        for cluster in ["Tp0", "Tp1", "Tp2", "Tpx"] {
            let samples = cluster_samples(&table, cluster);
            assert_eq!(
                hottest_when_differentiated(&samples),
                None,
                "{cluster}* answered {} distinct values across {} keys, which is the table not saying",
                distinct_values(&samples),
                samples.len(),
            );
        }
        // Why the decision cannot be made over all 132 keys at once: pooled, this
        // read has 25 distinct values against a required 66, so a single pooled
        // judgement would refuse the heat pipe along with the die. In 50 recorded
        // reads that pooling put ordinary real answers at 69 distinct — three
        // values above the line.
        let pooled: Vec<f32> = ["Tp0", "Tp1", "Tp2", "Tpx", "Ts0"]
            .iter()
            .flat_map(|cluster| cluster_samples(&table, cluster))
            .collect();
        assert_eq!(pooled.len(), 132);
        assert_eq!(
            hottest_when_differentiated(&pooled),
            None,
            "judged as one group, a live cluster dies with the silent ones"
        );
    }

    #[test]
    fn a_real_collapsed_window_is_not_a_temperature() {
        let table = parse_transcript(DIFFERENTIATED);
        let window = parse_transcript(COLLAPSED);
        assert!(
            window.keys.is_empty(),
            "the window file carries values only"
        );
        let replay = table.with_values_from(&window);
        // 45 keys on four values, each inside the plausible range. The hottest,
        // 40.0, is precisely what x printed to users before this rule existed.
        // This capture predates the widening: it recorded the `Tp0*` payloads
        // only, so it exercises one cluster rather than the whole set.
        assert_eq!(client(replay).cpu_temperature(), None);
    }

    #[test]
    fn a_cluster_that_has_not_differentiated_cannot_supply_the_answer() {
        // Hand-written, because the measured placeholder sets are cold (5.2 C and
        // below) and never the hottest thing on the machine: this is the shape
        // the refusal exists for even though this machine has not shown it. Six
        // `Tp1*` keys flat at 71.328125 against four `Tp0*` keys that spread.
        let transcript = parse_transcript(
            "
            COUNT 14
            KEY Tp01
            KEY Tp02
            KEY Tp03
            KEY Tp04
            KEY Tp1a
            KEY Tp1b
            KEY Tp1c
            KEY Tp1d
            KEY Tp1e
            KEY Tp1f
            KEY Ts01
            KEY Ts02
            KEY Tpx9
            KEY TG0P
            DATA #KEY 75693332 4 0000000e
            DATA Tp01 666c7420 4 00002042
            DATA Tp02 666c7420 4 00004c42
            DATA Tp03 666c7420 4 00007e42
            DATA Tp04 666c7420 4 00003542
            DATA Tp1a 666c7420 4 00a88e42
            DATA Tp1b 666c7420 4 00a88e42
            DATA Tp1c 666c7420 4 00a88e42
            DATA Tp1d 666c7420 4 00a88e42
            DATA Tp1e 666c7420 4 00a88e42
            DATA Tp1f 666c7420 4 00a88e42
            DATA Ts01 666c7420 4 00003042
            DATA Ts02 666c7420 4 00003842
            DATA Tpx9 666c7420 4 00007442
            DATA TG0P 666c7420 4 0000c642
            ",
        );
        // 63.5 from `Tp0*`, 46.0 from `Ts0*` and 61.0 from the single `Tpx*` key
        // all answer; the flat cluster is refused even though it holds the
        // highest number in the read, and the GPU sensor is never asked.
        assert_eq!(
            client(transcript).cpu_temperature(),
            Some(63.5),
            "the hottest trusted cluster wins, not the hottest payload"
        );
    }

    #[test]
    fn the_loop_decodes_each_type_and_refuses_the_rest() {
        // Hand-written, because no single machine carries all of these shapes at
        // once: an `flt ` core, an `sp78` core, a GPU sensor that must not be
        // read, a type with no decoder, a key that declares no size, a key that
        // declares more than the buffer holds.
        let transcript = parse_transcript(
            "
            COUNT 7
            KEY Tp01
            KEY TC0P
            KEY TG0P
            KEY Tp02
            KEY Tp03
            KEY Tp04
            KEY Tm0P
            DATA #KEY 75693332 4 00000007
            DATA Tp01 666c7420 4 0000c041
            DATA TC0P 73703738 2 2e00
            DATA TG0P 666c7420 4 0000c642
            DATA Tp02 696f6674 4 0000c041
            DATA Tp03 666c7420 0
            DATA Tp04 666c7420 40 00
            DATA Tm0P 666c7420 4 00004842
            ",
        );
        // 24.0 and 46.0 and 50.0 answer; 99.0 is the GPU and must not be in the
        // set at all, so a filter that widened by accident shows up here.
        assert_eq!(client(transcript).cpu_temperature(), Some(50.0));
    }

    #[test]
    fn a_key_the_table_does_not_have_is_not_invented() {
        // This machine is Apple silicon: `TC0P` is the Intel die sensor and is
        // absent, so its key-not-found answer has to become `None` rather than a
        // zero or a stale payload.
        let client = client(parse_transcript(DIFFERENTIATED));
        assert_eq!(client.read_key(pack_key("TC0P")), None);
        assert_eq!(client.key_at(99_999), None, "an index past the table");
    }

    #[test]
    fn a_sm_that_answers_for_no_cpu_key_says_nothing() {
        // The shape of a virtualised runner: the table enumerates, the sensors
        // refuse. Nothing is printed rather than something guessed.
        let mut table = parse_transcript(DIFFERENTIATED);
        table.infos.retain(|&key, _| key == pack_key("#KEY"));
        table.payloads.retain(|&key, _| key == pack_key("#KEY"));
        assert_eq!(client(table).cpu_temperature(), None);
    }

    #[test]
    fn a_transport_that_never_answers_says_nothing() {
        struct Dead;
        impl RoundTrip for Dead {
            fn round_trip(&self, _input: &SmcKeyData) -> Option<SmcKeyData> {
                None
            }
        }
        assert_eq!(SmcClient::new(Dead).cpu_temperature(), None);
    }

    #[test]
    fn a_key_count_that_is_not_a_key_count_is_not_enumerated() {
        let nine = parse_transcript("COUNT 99999\nKEY Tp01\nDATA #KEY 75693332 4 0001869f\n");
        assert_eq!(
            client(nine).cpu_temperature(),
            None,
            "a machine claiming 99999 keys is not an SMC"
        );
        let zero = parse_transcript("COUNT 0\nDATA #KEY 75693332 4 00000000\n");
        assert_eq!(client(zero).cpu_temperature(), None, "no keys at all");
    }
}
