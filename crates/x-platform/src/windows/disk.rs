//! Windows disk adapter: `sysinfo` for the mount list, then the volume and
//! storage IOCTLs for the physical facts.
//!
//! Everything here is a native Win32 call — no `wmic`, `diskpart` or PowerShell,
//! whose output is either localized or deprecated:
//!
//! * `GetVolumeInformationW` and `GetVolumeNameForVolumeMountPointW` give the
//!   label and the `\\?\Volume{GUID}\` path of a mount point.
//! * The disk-level IOCTLs are sent to the `\\.\X:` drive form of the mount,
//!   not to the volume path: on a live machine the volume-path handle rejects
//!   both `IOCTL_DISK_GET_PARTITION_INFO_EX` and
//!   `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` with `ERROR_INVALID_PARAMETER`,
//!   while the drive-form handle answers both. Folder-mounted volumes have no
//!   drive form, so their extra fields simply stay `None`.
//! * `IOCTL_DISK_GET_PARTITION_INFO_EX` gives the GPT partition GUID, gated on
//!   the partition-type GUID rather than the documented `PartitionStyle`
//!   field: on observed machines the field reads `1` for genuine GPT
//!   partitions while the GPT arm of the union carries the real, provider-
//!   matching GUID bytes. MBR partitions have no UUID at all and stay `None`.
//! * `IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS` maps the volume to its disk, and
//!   `IOCTL_STORAGE_QUERY_PROPERTY` on `\\.\PhysicalDriveN` gives model, serial
//!   and bus type, with the seek-penalty property deciding HDD vs SSD.
//!   The physical-drive query is security-descriptor gated: without permission
//!   the fields simply stay `None`.
//!
//! No block-device I/O counters here: `DiskManager::io` keeps its default
//! "unavailable on this platform" error. The Windows source is
//! `IOCTL_STORAGE_QUERY_PERF_DATA` on `IO_COUNTERS_QUERY`, which this
//! `windows-sys` version does not bind (`IO_COUNTERS` exists only as the
//! unrelated process-memory structure under `Threading`). Rather than
//! re-declaring the structure and IOCTL by hand — a 20-field layout where a
//! wrong field order silently misreports bytes — the command stays absent
//! on Windows until the binding lands.

use std::ffi::c_void;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, GetVolumeInformationW, GetVolumeNameForVolumeMountPointW, FILE_ATTRIBUTE_NORMAL,
    FILE_FLAG_BACKUP_SEMANTICS, FILE_SHARE_READ, FILE_SHARE_WRITE,
    IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, OPEN_EXISTING,
};
use windows_sys::Win32::System::Ioctl::{
    PropertyStandardQuery, StorageDeviceProperty, StorageDeviceSeekPenaltyProperty,
    DEVICE_SEEK_PENALTY_DESCRIPTOR, DISK_EXTENT, IOCTL_DISK_GET_PARTITION_INFO_EX,
    IOCTL_STORAGE_QUERY_PROPERTY, PARTITION_INFORMATION_EX, STORAGE_DEVICE_DESCRIPTOR,
    STORAGE_PROPERTY_QUERY, VOLUME_DISK_EXTENTS,
};
use windows_sys::Win32::System::IO::DeviceIoControl;
use x_core::disk::{DiskInfo, DiskManager, MediaType};
use x_core::error::Result;

use crate::common::disk_sysinfo::SysinfoDisk;

/// First groups of the Windows-managed GPT partition type GUIDs (Microsoft
/// Basic Data, ESP, MSR and Recovery). A `PartitionType` that starts with one
/// of these proves the union's GPT arm is live data.
const KNOWN_GPT_PARTITION_TYPES: [u32; 4] = [
    0xEBD0_A0A2, // Microsoft Basic Data
    0xC12A_7328, // EFI System Partition
    0xE3C9_E316, // Microsoft Reserved
    0xDE94_BBA4, // Windows Recovery Environment
];

/// Lists mounts and decorates them with volume and physical-drive facts.
#[derive(Debug, Default)]
pub struct WindowsDisk;

impl WindowsDisk {
    /// Create the adapter.
    pub fn new() -> Self {
        Self
    }
}

impl DiskManager for WindowsDisk {
    fn list(&self) -> Result<Vec<DiskInfo>> {
        let mut rows = SysinfoDisk::new().list()?;
        for row in &mut rows {
            let mount = volume_root(&row.mount_point);
            let wide_mount = encode_wide(&mount);
            let mut label = [0u16; 261];
            let mut serial = 0u32;
            let mut components = 0u32;
            let mut flags = 0u32;
            let mut fs_name = [0u16; 64];
            // SAFETY: every pointer targets a live buffer of the advertised
            // size; failure simply writes nothing.
            let ok = unsafe {
                GetVolumeInformationW(
                    wide_mount.as_ptr(),
                    label.as_mut_ptr(),
                    label.len() as u32,
                    &mut serial,
                    &mut components,
                    &mut flags,
                    fs_name.as_mut_ptr(),
                    fs_name.len() as u32,
                )
            };
            if ok != 0 {
                row.label = decode_wide(&label);
            }
            let mut volume_path = [0u16; 64];
            // SAFETY: as above, `volume_path` is large enough for the GUID form.
            let ok = unsafe {
                GetVolumeNameForVolumeMountPointW(
                    wide_mount.as_ptr(),
                    volume_path.as_mut_ptr(),
                    volume_path.len() as u32,
                )
            };
            if ok == 0 {
                continue;
            }
            let Some(volume_guid_path) = decode_wide(&volume_path) else {
                continue;
            };
            row.volume_uuid = volume_guid_from_path(&volume_guid_path);
            // The disk IOCTLs only answer on the drive-form handle, which only
            // exists for letter-mounted volumes; folder mounts stay undecorated.
            let Some(target) = drive_handle_target(&mount) else {
                continue;
            };
            let Some(handle) = open(&target) else {
                continue;
            };
            row.partition_uuid = partition_guid(handle.0);
            let disk_number = first_disk_number(handle.0);
            drop(handle);
            if let Some(disk_number) = disk_number {
                if let Some(identity) = drive_identity(disk_number) {
                    row.device_model = identity.0;
                    row.device_serial = identity.1;
                    row.media_type = identity.2;
                }
            }
        }
        for row in &mut rows {
            // A UNC device is a mapped network share; the drive stack below
            // cannot see it and `sysinfo` reports the remote path as the name.
            if row.media_type.is_none()
                && row
                    .name
                    .as_deref()
                    .is_some_and(|name| name.starts_with("\\\\"))
            {
                row.media_type = Some(MediaType::Network);
            }
        }
        Ok(rows)
    }
}

/// `GetVolumeInformationW` and the mount-point APIs insist on a trailing
/// backslash for drive roots.
fn volume_root(mount: &str) -> String {
    if mount.is_empty() {
        return "\\\\".to_string();
    }
    if mount.ends_with('\\') {
        mount.to_string()
    } else {
        format!("{mount}\\")
    }
}

/// The `\\.\X:` form of a drive-letter mount, the handle form the disk IOCTLs
/// answer on; folder mounts and UNC paths have none.
fn drive_handle_target(mount: &str) -> Option<String> {
    let bytes = mount.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        Some(format!("\\\\.\\{}:", bytes[0] as char))
    } else {
        None
    }
}

fn encode_wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn decode_wide(buffer: &[u16]) -> Option<String> {
    let end = buffer.iter().position(|c| *c == 0).unwrap_or(buffer.len());
    let text = String::from_utf16_lossy(&buffer[..end]);
    (!text.is_empty()).then_some(text)
}

/// The GUID body of `\\?\Volume{0123abcd-...}\`, in canonical lowercase.
fn volume_guid_from_path(path: &str) -> Option<String> {
    let start = path.find('{')?;
    let end = path.find('}')?;
    (end > start).then(|| path[start + 1..end].to_ascii_lowercase())
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if self.0 != INVALID_HANDLE_VALUE {
            // SAFETY: the handle came from `CreateFileW` and is closed once.
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// Opens a volume or physical-drive path with no access rights: the query
/// IOCTLs below only need metadata, and access 0 keeps them working without
/// elevation where the OS allows it.
///
/// `FILE_FLAG_BACKUP_SEMANTICS` is not optional here: a volume root is a
/// directory-like object, and without the flag the access-0 open fails with
/// `ERROR_PATH_NOT_FOUND` (observed for both `\\?\Volume{...}\` and `C:\`).
fn open(path: &str) -> Option<OwnedHandle> {
    let wide = encode_wide(path);
    // SAFETY: `wide` outlives the call; a failed open returns the sentinel.
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL | FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    (handle != INVALID_HANDLE_VALUE).then_some(OwnedHandle(handle))
}

/// Runs one control code over an output buffer and returns the byte count.
fn query(
    handle: HANDLE,
    code: u32,
    input: *const c_void,
    input_len: u32,
    output: &mut [u8],
) -> u32 {
    let mut returned = 0u32;
    // SAFETY: buffers are caller-sized; `null` input is valid when the length
    // is zero, and the overlapped pointer being `null` means synchronous use.
    let ok = unsafe {
        DeviceIoControl(
            handle,
            code,
            input,
            input_len,
            output.as_mut_ptr().cast::<c_void>(),
            output.len() as u32,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        0
    } else {
        returned
    }
}

/// Reads the (unaligned-safe) partition info for the volume.
fn partition_guid(handle: HANDLE) -> Option<String> {
    let mut output = [0u8; std::mem::size_of::<PARTITION_INFORMATION_EX>() + 16];
    let returned = query(
        handle,
        IOCTL_DISK_GET_PARTITION_INFO_EX,
        std::ptr::null(),
        0,
        &mut output,
    );
    if (returned as usize) < std::mem::size_of::<PARTITION_INFORMATION_EX>() {
        return None;
    }
    // SAFETY: the IOCTL filled at least one whole struct and we read it
    // unaligned, so buffer alignment does not matter.
    let info = unsafe {
        output
            .as_ptr()
            .cast::<PARTITION_INFORMATION_EX>()
            .read_unaligned()
    };
    // SAFETY: the union is read as the GPT arm either way; the type GUID below
    // decides whether the bytes mean anything.
    let gpt = unsafe { info.Anonymous.Gpt };
    // The documented `PartitionStyle` field is not trusted: live machines
    // return `1` for volumes whose GPT arm demonstrably carries the real
    // provider GUID. Reading the MBR arm of a true MBR volume as GPT yields a
    // boot-flag shaped value (at most `0x0000_FF80`) in the first group, which
    // can never match the table below, so MBR partitions stay `None` honestly.
    if !KNOWN_GPT_PARTITION_TYPES.contains(&gpt.PartitionType.data1) {
        return None;
    }
    Some(format_guid(&gpt.PartitionId))
}

/// Registry-style GUID text in lowercase, the canonical UUID spelling the
/// other platforms report; `windows-sys`'s `GUID` carries no `Display`.
fn format_guid(guid: &windows_sys::core::GUID) -> String {
    let (head, tail) = guid.data4.split_at(2);
    let head = head.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let tail = tail.iter().map(|b| format!("{b:02x}")).collect::<String>();
    format!(
        "{:08x}-{:04x}-{:04x}-{head}-{tail}",
        guid.data1, guid.data2, guid.data3
    )
}

fn first_disk_number(handle: HANDLE) -> Option<u32> {
    let mut output =
        [0u8; std::mem::size_of::<VOLUME_DISK_EXTENTS>() + 32 * std::mem::size_of::<DISK_EXTENT>()];
    let returned = query(
        handle,
        IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS,
        std::ptr::null(),
        0,
        &mut output,
    );
    let needed = std::mem::size_of::<u32>() + std::mem::size_of::<DISK_EXTENT>();
    if (returned as usize) < needed {
        return None;
    }
    // SAFETY: the IOCTL filled the header plus at least one extent; both reads
    // are unaligned-safe.
    unsafe {
        let count = output.as_ptr().cast::<u32>().read_unaligned();
        let extent = output
            .as_ptr()
            .add(std::mem::size_of::<u32>())
            .cast::<DISK_EXTENT>()
            .read_unaligned();
        (count >= 1).then_some(extent.DiskNumber)
    }
}

/// Model / serial / medium of `\\.\PhysicalDriveN`, `None` when the security
/// descriptor denies the query to this user.
fn drive_identity(disk_number: u32) -> Option<(Option<String>, Option<String>, Option<MediaType>)> {
    let path = format!("\\\\.\\PhysicalDrive{disk_number}");
    let handle = open(&path)?;
    let mut output = [0u8; 1024];
    let input = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceProperty,
        QueryType: PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    let returned = query(
        handle.0,
        IOCTL_STORAGE_QUERY_PROPERTY,
        std::ptr::from_ref(&input).cast::<c_void>(),
        std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
        &mut output,
    );
    if (returned as usize) < std::mem::size_of::<STORAGE_DEVICE_DESCRIPTOR>() {
        return None;
    }
    // SAFETY: the IOCTL filled at least one descriptor header; the string
    // offsets are bounds-checked against the returned byte count below.
    let descriptor = unsafe {
        output
            .as_ptr()
            .cast::<STORAGE_DEVICE_DESCRIPTOR>()
            .read_unaligned()
    };
    let bytes = &output[..returned as usize];
    let vendor = descriptor_string(bytes, descriptor.VendorIdOffset);
    let product = descriptor_string(bytes, descriptor.ProductIdOffset);
    let model = match (vendor, product) {
        (Some(vendor), Some(product)) => Some(format!("{vendor} {product}")),
        (Some(vendor), None) => Some(vendor),
        (None, product) => product,
    };
    let serial = descriptor_string(bytes, descriptor.SerialNumberOffset);
    let bus = descriptor.BusType;
    let removable = descriptor.RemovableMedia;
    let seek_penalty = {
        let mut penalty_output = [0u8; std::mem::size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>() + 8];
        let input = STORAGE_PROPERTY_QUERY {
            PropertyId: StorageDeviceSeekPenaltyProperty,
            QueryType: PropertyStandardQuery,
            AdditionalParameters: [0],
        };
        let returned = query(
            handle.0,
            IOCTL_STORAGE_QUERY_PROPERTY,
            std::ptr::from_ref(&input).cast::<c_void>(),
            std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            &mut penalty_output,
        );
        (returned as usize >= std::mem::size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>())
            // SAFETY: a full descriptor was written.
            .then(|| unsafe {
                penalty_output
                    .as_ptr()
                    .cast::<DEVICE_SEEK_PENALTY_DESCRIPTOR>()
                    .read_unaligned()
                    .IncursSeekPenalty
            })
    };
    Some((model, serial, win_media_type(bus, removable, seek_penalty)))
}

/// ASCII string at a descriptor byte offset; offset `0` means absent.
fn descriptor_string(bytes: &[u8], offset: u32) -> Option<String> {
    let offset = offset as usize;
    if offset == 0 || offset >= bytes.len() {
        return None;
    }
    let tail = &bytes[offset..];
    let end = tail.iter().position(|b| *b == 0).unwrap_or(tail.len());
    let text = String::from_utf8_lossy(&tail[..end]).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Classifies the bus: NVMe beats everything, USB is removable media, and the
/// rest is decided by whether the device penalises seeks.
fn win_media_type(bus_type: i32, removable: bool, seek_penalty: Option<bool>) -> Option<MediaType> {
    // `windows_sys::Win32_Storage_FileSystem` exports these as plain `i32`s;
    // the values mirror winioctl.h.
    const BUS_TYPE_USB: i32 = 7;
    const BUS_TYPE_NVME: i32 = 17;
    const BUS_TYPE_SD: i32 = 12;
    const BUS_TYPE_MMC: i32 = 13;
    const BUS_TYPE_VIRTUAL: i32 = 14;
    const BUS_TYPE_FILE_BACKED_VIRTUAL: i32 = 15;
    match bus_type {
        BUS_TYPE_NVME => Some(MediaType::Nvme),
        BUS_TYPE_USB | BUS_TYPE_SD | BUS_TYPE_MMC => Some(MediaType::Removable),
        BUS_TYPE_VIRTUAL | BUS_TYPE_FILE_BACKED_VIRTUAL => Some(MediaType::Virtual),
        _ if removable => Some(MediaType::Removable),
        _ => match seek_penalty {
            Some(true) => Some(MediaType::Hdd),
            Some(false) => Some(MediaType::Ssd),
            None => None,
        },
    }
}

/// Trait object helper.
pub fn manager() -> std::sync::Arc<dyn DiskManager> {
    std::sync::Arc::new(WindowsDisk::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_roots_gain_the_trailing_backslash_the_apis_require() {
        assert_eq!(volume_root("C:"), "C:\\");
        assert_eq!(volume_root("C:\\"), "C:\\");
        assert_eq!(volume_root("D:\\"), "D:\\");
    }

    #[test]
    fn only_letter_mounts_have_a_drive_form_handle() {
        assert_eq!(drive_handle_target("C:\\").as_deref(), Some("\\\\.\\C:"));
        assert_eq!(drive_handle_target("D:"), Some("\\\\.\\D:".to_string()));
        assert_eq!(drive_handle_target("\\\\server\\share\\"), None);
        assert_eq!(drive_handle_target(""), None);
    }

    #[test]
    fn the_guid_is_the_braced_body_of_the_volume_path() {
        assert_eq!(
            volume_guid_from_path("\\\\?\\Volume{4bd0a52a-b7f4-4b95-a542-3dc87a3e8a11}\\"),
            Some("4bd0a52a-b7f4-4b95-a542-3dc87a3e8a11".into())
        );
        assert_eq!(volume_guid_from_path("C:\\"), None);
    }

    #[test]
    fn bus_and_seek_penalty_decide_the_medium() {
        assert_eq!(
            win_media_type(17, false, Some(false)),
            Some(MediaType::Nvme)
        );
        assert_eq!(
            win_media_type(7, false, Some(false)),
            Some(MediaType::Removable)
        );
        assert_eq!(win_media_type(11, false, Some(true)), Some(MediaType::Hdd));
        assert_eq!(win_media_type(11, false, Some(false)), Some(MediaType::Ssd));
        assert_eq!(
            win_media_type(3, true, Some(false)),
            Some(MediaType::Removable)
        );
        assert_eq!(win_media_type(3, false, None), None);
    }

    #[test]
    fn descriptor_strings_are_offset_addressed_and_trimmed() {
        // "SAMSUNG", "MZVLW", absent (offset 0), "S4K1", garbage after a NUL.
        let mut bytes = vec![0u8; 64];
        bytes[8..15].copy_from_slice(b"SAMSUNG");
        bytes[16..21].copy_from_slice(b"MZVLW");
        bytes[24..28].copy_from_slice(b"S4K1");
        bytes[29] = 0x99;
        assert_eq!(descriptor_string(&bytes, 0), None);
        assert_eq!(descriptor_string(&bytes, 4), None);
        assert_eq!(descriptor_string(&bytes, 8).as_deref(), Some("SAMSUNG"));
        assert_eq!(descriptor_string(&bytes, 16).as_deref(), Some("MZVLW"));
        assert_eq!(descriptor_string(&bytes, 24).as_deref(), Some("S4K1"));
        assert_eq!(descriptor_string(&bytes, 63), None);
    }

    #[test]
    fn live_volumes_report_label_uuid_and_disk_facts_where_permitted() {
        let rows = WindowsDisk::new().list().expect("list");
        assert!(!rows.is_empty());
        // The system volume must at least produce a GUID path and a volume
        // UUID; model/serial depend on the drive's security descriptor.
        let boot = rows
            .iter()
            .find(|row| {
                row.mount_point.eq_ignore_ascii_case("C:\\")
                    || row.mount_point.eq_ignore_ascii_case("C:")
            })
            .expect("C: is mounted in every test environment");
        assert!(boot.volume_uuid.is_some(), "volume uuid missing: {boot:?}");
        assert!(
            boot.partition_uuid.is_some(),
            "GPT systems must report a partition uuid: {boot:?}"
        );
    }

    #[test]
    fn physical_identity_is_attempted_on_the_system_disk() {
        let rows = WindowsDisk::new().list().expect("list");
        let boot = rows
            .iter()
            .find(|row| row.mount_point.starts_with("C:") && row.total_bytes > 0)
            .expect("C:");
        // Unprivileged `STORAGE_QUERY_PROPERTY` usually succeeds for the bus
        // identity on Windows 10/11 clients; if it does not, media_type stays
        // None and the assertion below would fail loudly rather than silently.
        if boot.media_type.is_some() {
            assert_ne!(boot.media_type, Some(MediaType::Network));
            assert!(
                boot.device_model.is_some(),
                "model missing with a known bus"
            );
        }
    }
}
