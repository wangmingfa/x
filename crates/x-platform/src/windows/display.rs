//! Windows display topology through the Gdi32 device enumeration APIs.
//!
//! Layer 2 again: `EnumDisplayDevicesW` names the monitors attached to each
//! adapter, `EnumDisplaySettingsW(ENUM_CURRENT_SETTINGS)` gives the active
//! mode (size, refresh, desktop position) and `GetDpiForMonitor` the
//! effective scaling. PowerShell projections of the same facts
//! (`Win32_VideoController`) cannot be matched to monitors without guessing,
//! and the doctrine forbids the guess.

use std::sync::Arc;

use windows_sys::Win32::Foundation::POINTL;
use windows_sys::Win32::Graphics::Gdi::{
    EnumDisplayDevicesW, EnumDisplaySettingsW, MonitorFromPoint, DEVMODEW, DISPLAY_DEVICEW,
    DISPLAY_DEVICE_ATTACHED_TO_DESKTOP, DISPLAY_DEVICE_PRIMARY_DEVICE, ENUM_CURRENT_SETTINGS,
    MONITOR_DEFAULTTONEAREST,
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
                out.push(info);
            }
        }
    }
    out
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
