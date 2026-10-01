//! Windows windows through the user32 window enumeration.
//!
//! Layer 2: `EnumWindows` walks the top-level windows, `GetWindowTextW` names
//! them, `GetWindowThreadProcessId` plus `QueryFullProcessImageNameW` attribute
//! them to a process, `IsIconic` / `IsZoomed` / `GetForegroundWindow` report
//! the three states and `ShowWindow` + `SetForegroundWindow` move them. No
//! locale-sensitive command output is parsed anywhere.
//!
//! The list keeps only windows that are visible and carry a non-empty title:
//! message-only and helper windows are noise, and the doctrine is to show
//! facts, not every handle the system owns.

use std::sync::Arc;

use windows_sys::Win32::Foundation::{HWND, LPARAM, TRUE};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW,
    GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, IsZoomed, SetForegroundWindow,
    ShowWindow, SW_MAXIMIZE, SW_MINIMIZE, SW_RESTORE,
};
use x_core::error::{Error, ErrorKind, Result};
use x_core::window::{WindowInfo, WindowManager};

/// Adapter installed into the composition root.
pub struct PlatformWindows;

/// Arc ready for the builder.
pub fn manager() -> Arc<dyn WindowManager> {
    Arc::new(PlatformWindows)
}

impl WindowManager for PlatformWindows {
    fn windows(&self) -> Result<Vec<WindowInfo>> {
        Ok(enumerate())
    }

    fn active(&self) -> Result<Option<WindowInfo>> {
        // SAFETY: GetForegroundWindow has no preconditions; null means no
        // window currently holds the foreground.
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.is_null() {
            return Ok(None);
        }
        Ok(Some(describe(hwnd)))
    }

    fn focus(&self, window: &WindowInfo) -> Result<()> {
        let hwnd = live_handle(window)?;
        // SAFETY: the handle was just validated by IsWindow; both calls are
        // safe on a live window.
        let raised = unsafe {
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
            SetForegroundWindow(hwnd)
        };
        if raised == 0 {
            return Err(Error::new(
                ErrorKind::InvalidState,
                format!(
                    "the system refused to bring {} to the foreground; another window holds the foreground lock — retry, or click the window once first",
                    window.label()
                ),
            ));
        }
        Ok(())
    }

    fn minimize(&self, window: &WindowInfo) -> Result<()> {
        let hwnd = live_handle(window)?;
        // SAFETY: live handle; ShowWindow's return value reports the previous
        // visibility, not success, so it is not checked.
        unsafe { ShowWindow(hwnd, SW_MINIMIZE) };
        Ok(())
    }

    fn maximize(&self, window: &WindowInfo) -> Result<()> {
        let hwnd = live_handle(window)?;
        // SAFETY: same as `minimize`.
        unsafe { ShowWindow(hwnd, SW_MAXIMIZE) };
        Ok(())
    }
}

/// Handles collected by the `EnumWindows` callback.
///
/// SAFETY: the callback only appends to the vector behind `lparam`, which the
/// caller keeps alive for the whole enumeration.
unsafe extern "system" fn collect_visible(hwnd: HWND, lparam: LPARAM) -> windows_sys::core::BOOL {
    let out = &mut *(lparam as *mut Vec<isize>);
    if IsWindowVisible(hwnd) != 0 {
        out.push(hwnd as isize);
    }
    TRUE
}

fn enumerate() -> Vec<WindowInfo> {
    let mut handles: Vec<isize> = Vec::new();
    // SAFETY: `collect_visible` matches the WNDENUMPROC contract and the
    // vector behind the pointer outlives the call.
    unsafe {
        EnumWindows(
            Some(collect_visible),
            &mut handles as *mut Vec<isize> as LPARAM,
        );
    }
    handles
        .into_iter()
        .map(|handle| describe(handle as HWND))
        .filter(|info| info.title.is_some())
        .collect()
}

/// Every fact the Win32 side can supply for one window.
fn describe(hwnd: HWND) -> WindowInfo {
    let mut info = WindowInfo {
        title: None,
        id: None,
        pid: None,
        app: None,
        active: None,
        minimized: None,
        maximized: None,
    };
    // SAFETY: every call inspects `hwnd` only; a handle that dies mid-walk
    // makes the calls fail, which each branch treats as "not known".
    unsafe {
        info.title = window_title(hwnd);
        info.id = Some(format!("0x{:X}", hwnd as isize));
        let mut pid: u32 = 0;
        if GetWindowThreadProcessId(hwnd, &mut pid) != 0 && pid != 0 {
            info.pid = Some(pid);
            info.app = app_name(pid);
        }
        info.active = Some(hwnd == GetForegroundWindow());
        info.minimized = Some(IsIconic(hwnd) != 0);
        info.maximized = Some(IsZoomed(hwnd) != 0);
    }
    info
}

/// Title of a window, `None` when it has none.
fn window_title(hwnd: HWND) -> Option<String> {
    // SAFETY: both calls take the handle and a buffer sized from the length
    // the first call reported.
    unsafe {
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return None;
        }
        let mut buffer = vec![0u16; len as usize + 1];
        let copied = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
        if copied <= 0 {
            return None;
        }
        let title = String::from_utf16_lossy(&buffer[..copied as usize]);
        (!title.is_empty()).then_some(title)
    }
}

/// Executable stem of the process that owns a window (`explorer`, `chrome`).
fn app_name(pid: u32) -> Option<String> {
    // SAFETY: the process handle is owned by `OwnedHandle`; both calls are
    // checked and the buffer starts at a generous constant size.
    unsafe {
        let process = crate::windows::OwnedHandle::new(OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION,
            0,
            pid,
        ))?;
        let mut buffer = vec![0u16; 1024];
        let mut size = buffer.len() as u32;
        if QueryFullProcessImageNameW(process.0, 0, buffer.as_mut_ptr(), &mut size) == 0 {
            return None;
        }
        let path = String::from_utf16_lossy(&buffer[..size as usize]);
        let stem = std::path::Path::new(&path)
            .file_stem()?
            .to_string_lossy()
            .into_owned();
        (!stem.is_empty()).then_some(stem)
    }
}

/// Handle carried by a listed row, re-parsed from its `0x…` id.
fn handle_of(window: &WindowInfo) -> Result<HWND> {
    let id = window.id.as_deref().ok_or_else(|| {
        Error::invalid_input("this window row carries no handle; re-run `x window list`")
    })?;
    let hex = id
        .strip_prefix("0x")
        .or_else(|| id.strip_prefix("0X"))
        .unwrap_or(id);
    let value = usize::from_str_radix(hex, 16)
        .map_err(|_| Error::invalid_input(format!("{id:?} is not a window handle")))?;
    Ok(value as HWND)
}

/// [`handle_of`] plus a liveness check, so verbs on a closed window say so.
fn live_handle(window: &WindowInfo) -> Result<HWND> {
    let hwnd = handle_of(window)?;
    // SAFETY: IsWindow only inspects the handle value.
    if unsafe { IsWindow(hwnd) } == 0 {
        return Err(Error::not_found(format!(
            "{} no longer exists; re-run `x window list`",
            window.label()
        )));
    }
    Ok(hwnd)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: Option<&str>) -> WindowInfo {
        let mut info = WindowInfo::new("Editor");
        info.id = id.map(str::to_string);
        info
    }

    #[test]
    fn handles_round_trip_through_their_hex_ids() {
        let hwnd = handle_of(&row(Some("0x1A2B"))).expect("parse");
        assert_eq!(hwnd as usize, 0x1A2B);
        // The description formats with `{:X}`, so the parse must accept it.
        assert_eq!(
            handle_of(&row(Some("0x1a2b"))).expect("parse") as usize,
            0x1A2B
        );
    }

    #[test]
    fn missing_or_malformed_ids_are_rejected() {
        assert!(handle_of(&row(None)).is_err());
        assert!(handle_of(&row(Some("not a handle"))).is_err());
    }

    #[test]
    fn the_live_enumeration_never_lies_about_ids() {
        // Runs on a real desktop: every listed row must carry a handle and a
        // title, because those are the filters.
        for info in enumerate() {
            assert!(info.id.is_some(), "{info:?}");
            assert!(info.title.is_some(), "{info:?}");
        }
    }
}
