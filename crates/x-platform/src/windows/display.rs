//! Windows display topology through the Gdi32 device enumeration APIs.
//!
//! Layer 2 again: `EnumDisplayDevicesW` names the monitors attached to each
//! adapter, `EnumDisplaySettingsW(ENUM_CURRENT_SETTINGS)` gives the active
//! mode (size, refresh, desktop position) and `GetDpiForMonitor` the
//! effective scaling. PowerShell projections of the same facts
//! (`Win32_VideoController`) cannot be matched to monitors without guessing,
//! and the doctrine forbids the guess.
//!
//! HDR is the one fact GDI does not carry, so it comes from DisplayConfig
//! (`DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO`), which is authoritative for
//! whether HDR is *active right now*. A failure there leaves the field `None`
//! rather than `false`: "the API would not say" is not "HDR is off".

use std::collections::HashMap;
use std::sync::Arc;

use windows_sys::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
    DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
    DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO, DISPLAYCONFIG_MODE_INFO, DISPLAYCONFIG_PATH_INFO,
    DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TOPOLOGY_ID, QDC_ONLY_ACTIVE_PATHS,
};
use windows_sys::Win32::Foundation::POINTL;
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayDevicesW, EnumDisplaySettingsW, MonitorFromPoint, DEVMODEW,
    DISPLAYCONFIG_COLOR_ENCODING_RGB, DISPLAY_DEVICEW, DISPLAY_DEVICE_ATTACHED_TO_DESKTOP,
    DISPLAY_DEVICE_PRIMARY_DEVICE, ENUM_CURRENT_SETTINGS, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use x_core::display::{DisplayInfo, DisplayManager, Point, Resolution};
use x_core::error::Result;

/// Adapter installed into the composition root.
pub struct PlatformDisplays;

/// Arc ready for the builder.
pub fn manager() -> Arc<dyn DisplayManager> {
    Arc::new(PlatformDisplays)
}

impl DisplayManager for PlatformDisplays {
    fn displays(&self) -> Result<Vec<DisplayInfo>> {
        Ok(enumerate())
    }
}

fn enumerate() -> Vec<DisplayInfo> {
    let mut out = Vec::new();
    // HDR lives in DisplayConfig, which indexes by adapter LUID + source id
    // rather than by the `\\.\DISPLAYn` name GDI reports, so the states are
    // gathered once here and joined below.
    let hdr = hdr_states();
    // Two nested walks; 64 is far above any real fan-out and keeps a
    // misbehaving driver from looping forever.
    for adapter_index in 0..64u32 {
        // SAFETY: zeroed Win32 structs are valid starting states; every call
        // below checks its return value before reading what it filled in.
        unsafe {
            let mut adapter: DISPLAY_DEVICEW = std::mem::zeroed();
            adapter.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as u32;
            if EnumDisplayDevicesW(std::ptr::null(), adapter_index, &mut adapter, 0) == 0 {
                break;
            }
            let adapter_name = utf16_to_string(&adapter.DeviceName);
            if adapter_name.is_empty() {
                continue;
            }
            let primary = adapter.StateFlags & DISPLAY_DEVICE_PRIMARY_DEVICE != 0;

            let mut mode: DEVMODEW = std::mem::zeroed();
            mode.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
            let mode_ok = EnumDisplaySettingsW(
                adapter.DeviceName.as_ptr(),
                ENUM_CURRENT_SETTINGS,
                &mut mode,
            ) != 0;

            for monitor_index in 0..64u32 {
                let mut monitor: DISPLAY_DEVICEW = std::mem::zeroed();
                monitor.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as u32;
                if EnumDisplayDevicesW(adapter.DeviceName.as_ptr(), monitor_index, &mut monitor, 0)
                    == 0
                {
                    break;
                }
                let name = utf16_to_string(&monitor.DeviceString);
                if name.is_empty() {
                    continue;
                }
                let mut info = DisplayInfo::new(name);
                info.id = Some(adapter_name.clone());
                info.connected = Some(monitor.StateFlags & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP != 0);
                info.primary = Some(primary);
                if mode_ok {
                    if mode.dmPelsWidth > 0 && mode.dmPelsHeight > 0 {
                        info.resolution = Some(Resolution {
                            width: mode.dmPelsWidth,
                            height: mode.dmPelsHeight,
                        });
                    }
                    if mode.dmDisplayFrequency > 0 {
                        info.refresh_hz = Some(mode.dmDisplayFrequency as f64);
                    }
                    let origin: POINTL = mode.Anonymous1.Anonymous2.dmPosition;
                    let position = Point {
                        x: origin.x,
                        y: origin.y,
                    };
                    info.position = Some(position);
                    info.scale_percent = effective_scale(position);
                }
                // Keyed by the GDI device name, the one string both APIs agree
                // on. A display with no entry keeps `None` — "nothing said" —
                // which is not the claim "HDR is off".
                info.hdr_enabled = hdr.get(&adapter_name).copied();
                out.push(info);
            }
        }
    }
    out
}

/// Whether HDR is active on each display, keyed by GDI device name
/// (`\\.\DISPLAY1`).
///
/// DisplayConfig is the only Windows API that says whether HDR is *on right
/// now*; GDI's `DEVMODE` has no such field. The bit lives in
/// `GET_ADVANCED_COLOR_INFO`'s bitfield and is meaningful only while the color
/// encoding is actually RGB.
///
/// The join key is the source's `viewGdiDeviceName`, because that is exactly
/// the string `EnumDisplayDevicesW` reports — DisplayConfig's own LUID-based
/// ids are not available to the GDI walk, so keying on them would silently
/// match nothing.
///
/// Every failure path returns an empty map rather than a `false`: an absent
/// entry leaves `hdr_enabled` as `None`, which is "nothing said", and is not
/// the same claim as "HDR is off".
fn hdr_states() -> HashMap<String, bool> {
    let mut out = HashMap::new();
    // SAFETY: every call below checks its return value, and both buffers are
    // sized by the API itself before they are filled.
    unsafe {
        let mut path_count: u32 = 0;
        let mut mode_count: u32 = 0;
        // QDC_ONLY_ACTIVE_PATHS: only paths that are currently driving a
        // display, which is what "is HDR on" about. Virtual and inactive paths
        // would answer for hardware nobody is looking at.
        if GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count) != 0
        {
            return out;
        }
        if path_count == 0 {
            return out;
        }

        let mut paths: Vec<DISPLAYCONFIG_PATH_INFO> = vec![std::mem::zeroed(); path_count as usize];
        let mut modes: Vec<DISPLAYCONFIG_MODE_INFO> = vec![std::mem::zeroed(); mode_count as usize];
        let mut topology: DISPLAYCONFIG_TOPOLOGY_ID = std::mem::zeroed();
        let rc = QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut path_count,
            paths.as_mut_ptr(),
            &mut mode_count,
            modes.as_mut_ptr(),
            &mut topology,
        );
        if rc != 0 {
            return out;
        }

        for path in paths.iter().take(path_count as usize) {
            let mut info: DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO = std::mem::zeroed();
            info.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_ADVANCED_COLOR_INFO;
            info.header.size = std::mem::size_of::<DISPLAYCONFIG_GET_ADVANCED_COLOR_INFO>() as u32;
            info.header.adapterId = path.sourceInfo.adapterId;
            info.header.id = path.sourceInfo.id;
            if DisplayConfigGetDeviceInfo(&mut info.header as *mut _ as *mut _) != 0 {
                continue;
            }
            // A non-RGB encoding means no advanced color state is in play, so
            // the bit is not a fact about HDR and must not be read as one.
            if info.colorEncoding != DISPLAYCONFIG_COLOR_ENCODING_RGB {
                continue;
            }
            let gdi_name = source_gdi_name(
                (
                    path.sourceInfo.adapterId.LowPart,
                    path.sourceInfo.adapterId.HighPart,
                ),
                path.sourceInfo.id,
            );
            let Some(gdi_name) = gdi_name else {
                continue;
            };
            // Bit 0 of the bitfield is `advancedColorEnabled`.
            out.insert(gdi_name, info.Anonymous.value & 1 != 0);
        }
    }
    out
}

/// The GDI device name of one source, the same string `EnumDisplayDevicesW`
/// reports as `DeviceName`.
///
/// Without this the HDR states cannot be joined to the enumerated displays at
/// all, so a machine that reports HDR correctly would still show nothing.
fn source_gdi_name(adapter: (u32, i32), id: u32) -> Option<String> {
    let mut name: DISPLAYCONFIG_SOURCE_DEVICE_NAME = unsafe { std::mem::zeroed() };
    name.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
    name.header.size = std::mem::size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
    name.header.adapterId = windows_sys::Win32::Foundation::LUID {
        LowPart: adapter.0,
        HighPart: adapter.1,
    };
    name.header.id = id;
    // SAFETY: `name` is our own zeroed storage with a correct type and size.
    if unsafe { DisplayConfigGetDeviceInfo(&mut name.header as *mut _ as *mut _) } != 0 {
        return None;
    }
    let text = utf16_to_string(&name.viewGdiDeviceName);
    (!text.is_empty()).then_some(text)
}

/// Effective DPI of the monitor nearest this desktop point, as a percentage.
///
/// SAFETY-free wrapper: the Win32 calls take only a value point and plain
/// out-params into stack locals.
fn effective_scale(position: Point) -> Option<u32> {
    unsafe {
        let point = windows_sys::Win32::Foundation::POINT {
            x: position.x,
            y: position.y,
        };
        let monitor = MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST);
        if monitor.is_null() {
            return None;
        }
        let (mut dpi_x, mut dpi_y) = (0u32, 0u32);
        if GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) != 0 {
            return None;
        }
        // 96 DPI is 100 %; a zero answer is not a fact.
        if dpi_x == 0 {
            return None;
        }
        Some(((dpi_x as f64) / 96.0 * 100.0).round() as u32)
    }
}

/// UTF-16 buffer prefix up to the first NUL.
fn utf16_to_string(buffer: &[u16]) -> String {
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    String::from_utf16_lossy(&buffer[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hdr_states_join_the_gdi_names_the_enumeration_reports() {
        // The HDR states are keyed by `viewGdiDeviceName` precisely because
        // that is the string `EnumDisplayDevicesW` hands back. If those two
        // ever stop agreeing the join matches nothing and every display would
        // silently report "unknown" forever — indistinguishable from a machine
        // full of SDR panels, which is exactly the failure worth catching.
        let rows = enumerate();
        let states = hdr_states();
        if states.is_empty() {
            // No HDR path reported at all (SDR hardware, or the API refused);
            // then there is nothing to join and nothing to prove.
            assert!(
                rows.iter().all(|info| info.hdr_enabled.is_none()),
                "no HDR states, so no display may claim one"
            );
            return;
        }
        for info in &rows {
            let name = info.id.as_deref().unwrap_or_default();
            assert!(
                states.contains_key(name),
                "DisplayConfig names {name:?} but the HDR map holds {:?}",
                states.keys().collect::<Vec<_>>()
            );
        }
    }
}
