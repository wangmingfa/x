//! Linux and macOS windows.
//!
//! - **Linux** — X11 through `wmctrl` / `xprop` (layer 3; the X server has no
//!   stable Rust binding in this workspace and EWMH is what window managers
//!   speak): `wmctrl -lp` lists managed windows with their pid, `xprop`
//!   supplies the `_NET_WM_STATE` atoms and `_NET_ACTIVE_WINDOW`. The verbs
//!   are `wmctrl -i -a` (focus), `-b add,hidden` (minimize) and
//!   `-b add,maximized_*` (maximize). Without `DISPLAY` or without wmctrl the
//!   adapter fails honestly.
//! - **macOS** — the inventory comes from `CGWindowListCopyWindowInfo`
//!   (layer 2, CoreGraphics): pid, owner name, window number, and titles when
//!   the Screen Recording permission is granted. The verbs and the active
//!   window go through System Events UI scripting (`osascript`), which needs
//!   the Accessibility permission — its refusal is passed through verbatim.
//!
//! Reads are never audited; the verbs audit at the decorator layer.

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::process::Command;

#[cfg(target_os = "macos")]
use core_foundation::array::{CFArray, CFArrayRef};
#[cfg(target_os = "macos")]
use core_foundation::base::{CFType, TCFType};
#[cfg(target_os = "macos")]
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
#[cfg(target_os = "macos")]
use core_foundation::number::CFNumber;
#[cfg(target_os = "macos")]
use core_foundation::string::CFString;
use x_core::error::Error;
use x_core::error::Result;
use x_core::window::{WindowInfo, WindowManager};

/// Adapter installed into the composition root (Linux / macOS; Windows uses
/// the native user32 walk in `windows::window`).
#[derive(Debug, Default)]
pub struct PlatformWindows;

impl WindowManager for PlatformWindows {
    fn windows(&self) -> Result<Vec<WindowInfo>> {
        #[cfg(target_os = "linux")]
        {
            linux_windows()
        }
        #[cfg(target_os = "macos")]
        {
            macos_windows()
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            Err(Error::unsupported(
                "no window enumerator is compiled in for this platform",
            ))
        }
    }

    fn active(&self) -> Result<Option<WindowInfo>> {
        #[cfg(target_os = "linux")]
        {
            linux_active()
        }
        #[cfg(target_os = "macos")]
        {
            macos_active()
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            Err(Error::unsupported(
                "no window enumerator is compiled in for this platform",
            ))
        }
    }

    fn focus(&self, window: &WindowInfo) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            linux_focus(window)
        }
        #[cfg(target_os = "macos")]
        {
            macos_focus(window)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = window;
            Err(Error::unsupported(
                "no window control is compiled in for this platform",
            ))
        }
    }

    fn minimize(&self, window: &WindowInfo) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            linux_minimize(window)
        }
        #[cfg(target_os = "macos")]
        {
            macos_minimize(window)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = window;
            Err(Error::unsupported(
                "no window control is compiled in for this platform",
            ))
        }
    }

    fn maximize(&self, window: &WindowInfo) -> Result<()> {
        #[cfg(target_os = "linux")]
        {
            linux_maximize(window)
        }
        #[cfg(target_os = "macos")]
        {
            macos_maximize(window)
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = window;
            Err(Error::unsupported(
                "no window control is compiled in for this platform",
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Linux: wmctrl + xprop
// ---------------------------------------------------------------------------

/// `wmctrl` and `xprop` speak X11; a bare Wayland session has neither.
#[cfg(target_os = "linux")]
fn x11_display() -> Result<()> {
    if std::env::var_os("DISPLAY").is_none() {
        return Err(Error::unsupported(
            "no X11 display (DISPLAY is unset); x window drives wmctrl and xprop, which speak X11 only",
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn wmctrl(args: &[&str]) -> Result<String> {
    x11_display()?;
    let output = Command::new("wmctrl").args(args).output().map_err(|_| {
        Error::unsupported(
            "wmctrl is not installed; install the wmctrl package to manage X11 windows",
        )
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        return Err(Error::system(if stderr.is_empty() {
            format!(
                "wmctrl {} failed (exit {:?})",
                args.join(" "),
                output.status.code()
            )
        } else {
            format!("wmctrl {} failed: {stderr}", args.join(" "))
        }));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(target_os = "linux")]
fn xprop(args: &[&str]) -> Result<String> {
    x11_display()?;
    let output = Command::new("xprop").args(args).output().map_err(|_| {
        Error::unsupported(
            "xprop is not installed; install the x11-utils package for per-window state",
        )
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        return Err(Error::system(if stderr.is_empty() {
            format!(
                "xprop {} failed (exit {:?})",
                args.join(" "),
                output.status.code()
            )
        } else {
            format!("xprop {} failed: {stderr}", args.join(" "))
        }));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(target_os = "linux")]
fn linux_windows() -> Result<Vec<WindowInfo>> {
    let mut rows = parse_wmctrl(&wmctrl(&["-lp"])?);
    let active = ewmh_active_window()?;
    for row in &mut rows {
        enrich_states(row);
        if let Some(active) = active.as_deref() {
            row.active = Some(row.id.as_deref() == Some(active));
        }
    }
    Ok(rows)
}

#[cfg(target_os = "linux")]
fn linux_active() -> Result<Option<WindowInfo>> {
    let Some(id) = ewmh_active_window()? else {
        return Ok(None);
    };
    let found = parse_wmctrl(&wmctrl(&["-lp"])?)
        .into_iter()
        .find(|row| row.id.as_deref() == Some(id.as_str()));
    let mut row = match found {
        Some(mut row) => {
            enrich_states(&mut row);
            row
        }
        // The compositor named a window wmctrl does not manage; the id and
        // the focus fact are still real.
        None => WindowInfo {
            title: None,
            id: Some(id),
            pid: None,
            app: None,
            active: None,
            minimized: None,
            maximized: None,
        },
    };
    row.active = Some(true);
    Ok(Some(row))
}

/// `_NET_ACTIVE_WINDOW` of the root window, canonical `0x…` form.
#[cfg(target_os = "linux")]
fn ewmh_active_window() -> Result<Option<String>> {
    let text = xprop(&["-root", "_NET_ACTIVE_WINDOW"])?;
    Ok(parse_active(&text))
}

/// One `wmctrl -lp` line: `0x… id, desktop, pid, host, title`.
#[cfg(target_os = "linux")]
pub fn parse_wmctrl(text: &str) -> Vec<WindowInfo> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some((id, rest)) = take_token(line) else {
            continue;
        };
        let Some(id) = canonical_id(id) else {
            continue;
        };
        let Some((_desktop, rest)) = take_token(rest) else {
            continue;
        };
        let Some((pid, rest)) = take_token(rest) else {
            continue;
        };
        let Some((_host, title)) = take_token(rest) else {
            continue;
        };
        let title = title.trim();
        let pid = pid.parse::<u32>().ok();
        out.push(WindowInfo {
            title: (!title.is_empty()).then(|| title.to_string()),
            id: Some(id),
            app: pid.and_then(process_name),
            pid,
            active: None,
            minimized: None,
            maximized: None,
        });
    }
    out
}

/// `0x02a00004` → canonical uppercase `0x02A00004`; `None` for non-hex ids.
///
/// wmctrl pads and xprop does not, so every id is normalized before any
/// comparison happens.
#[cfg(target_os = "linux")]
fn canonical_id(id: &str) -> Option<String> {
    let hex = id
        .trim()
        .strip_prefix("0x")
        .or_else(|| id.trim().strip_prefix("0X"))?;
    let value = u64::from_str_radix(hex, 16).ok()?;
    Some(format!("0x{value:08X}"))
}

/// `_NET_ACTIVE_WINDOW(WINDOW): window id # 0x2a00004` → canonical id.
/// `0x0` is EWMH's "no window" and reads as `None`.
#[cfg(target_os = "linux")]
pub fn parse_active(text: &str) -> Option<String> {
    let (_, right) = text.split_once("window id #")?;
    let id = canonical_id(right.trim())?;
    (id != "0x00000000").then_some(id)
}

/// `_NET_WM_STATE(ATOM) = A, B` → `["A", "B"]`; `not found.` → empty.
#[cfg(target_os = "linux")]
pub fn parse_state_atoms(text: &str) -> Vec<String> {
    let Some((_, right)) = text.split_once('=') else {
        return Vec::new();
    };
    right
        .split(',')
        .map(str::trim)
        .filter(|atom| !atom.is_empty())
        .map(str::to_string)
        .collect()
}

/// Fill in the states `xprop` knows; unavailable xprop leaves them absent.
#[cfg(target_os = "linux")]
fn enrich_states(row: &mut WindowInfo) {
    let Some(id) = row.id.clone() else {
        return;
    };
    let Ok(text) = xprop(&["-id", id.as_str(), "_NET_WM_STATE"]) else {
        return;
    };
    let atoms = parse_state_atoms(&text);
    row.minimized = Some(atoms.iter().any(|atom| atom == "_NET_WM_STATE_HIDDEN"));
    // A vertical- or horizontal-only maximize is still the WM's maximize
    // state; both atoms are set by the usual window managers.
    row.maximized = Some(
        atoms
            .iter()
            .any(|atom| atom.ends_with("MAXIMIZED_VERT") || atom.ends_with("MAXIMIZED_HORZ")),
    );
}

/// Kernel name of the process holding a window (`comm`, 15 char limit).
#[cfg(target_os = "linux")]
fn process_name(pid: u32) -> Option<String> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/comm")).ok()?;
    let name = text.trim();
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(target_os = "linux")]
fn linux_focus(window: &WindowInfo) -> Result<()> {
    let id = require_id(window)?;
    wmctrl(&["-i", "-a", id.as_str()]).map(|_| ())
}

#[cfg(target_os = "linux")]
fn linux_minimize(window: &WindowInfo) -> Result<()> {
    let id = require_id(window)?;
    wmctrl(&["-i", "-r", id.as_str(), "-b", "add,hidden"]).map(|_| ())
}

#[cfg(target_os = "linux")]
fn linux_maximize(window: &WindowInfo) -> Result<()> {
    let id = require_id(window)?;
    wmctrl(&[
        "-i",
        "-r",
        id.as_str(),
        "-b",
        "add,maximized_vert,maximized_horz",
    ])
    .map(|_| ())
}

#[cfg(target_os = "linux")]
fn require_id(window: &WindowInfo) -> Result<String> {
    window.id.clone().ok_or_else(|| {
        Error::invalid_input("this window row carries no X11 id; re-run `x window list`")
    })
}

/// First whitespace-separated token and the remainder.
#[cfg(target_os = "linux")]
fn take_token(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start();
    if text.is_empty() {
        return None;
    }
    let end = text.find(char::is_whitespace).unwrap_or(text.len());
    Some((&text[..end], &text[end..]))
}

// ---------------------------------------------------------------------------
// macOS: CGWindowListCopyWindowInfo + System Events
// ---------------------------------------------------------------------------

/// The CoreGraphics window list: normal application windows, on screen.
#[cfg(target_os = "macos")]
const K_CG_WINDOW_LIST_ON_SCREEN_ONLY: u32 = 1 << 0;
#[cfg(target_os = "macos")]
const K_CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS: u32 = 1 << 4;
#[cfg(target_os = "macos")]
const K_CG_NULL_WINDOW_ID: u32 = 0;

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> CFArrayRef;
}

/// Sentinel the active-window script prints when the frontmost application
/// has no focused window, so an unnamed window is not confused with none.
#[cfg(target_os = "macos")]
const NO_FOCUSED_WINDOW: &str = "__no_focused_window__";

#[cfg(target_os = "macos")]
fn macos_windows() -> Result<Vec<WindowInfo>> {
    // SAFETY: CGWindowListCopyWindowInfo follows the CF create rule; the wrap
    // takes ownership, so the array is released exactly once. Every element of
    // this list is a CFDictionary.
    let list: CFArray = unsafe {
        let raw = CGWindowListCopyWindowInfo(
            K_CG_WINDOW_LIST_ON_SCREEN_ONLY | K_CG_WINDOW_LIST_EXCLUDE_DESKTOP_ELEMENTS,
            K_CG_NULL_WINDOW_ID,
        );
        if raw.is_null() {
            return Err(Error::system(
                "CGWindowListCopyWindowInfo returned no window list",
            ));
        }
        TCFType::wrap_under_create_rule(raw)
    };

    let mut out = Vec::new();
    for raw in list.get_all_values() {
        // SAFETY: wrap_under_get_rule retains and will release; the raw
        // pointer is owned by `list`, which outlives the dictionary.
        let dict: CFDictionary<CFString, CFType> =
            unsafe { TCFType::wrap_under_get_rule(raw as CFDictionaryRef) };
        // Layer 0 is the normal application windows; menu bar, Dock, tooltips
        // and overlays live on other layers.
        if number(&dict, "kCGWindowLayer") != Some(0) {
            continue;
        }
        let mut info = WindowInfo {
            title: None,
            id: None,
            pid: None,
            app: None,
            active: None,
            minimized: None,
            maximized: None,
        };
        if let Some(id) = number(&dict, "kCGWindowNumber") {
            info.id = Some(format!("0x{id:08X}"));
        }
        info.pid = number(&dict, "kCGWindowOwnerPID").map(|pid| pid as u32);
        info.app = string(&dict, "kCGWindowOwnerName");
        // Without the Screen Recording permission macOS replaces the title
        // with an empty string; empty is not a title.
        info.title = string(&dict, "kCGWindowName");
        out.push(info);
    }
    Ok(out)
}

/// The frontmost application's focused window, through System Events.
#[cfg(target_os = "macos")]
fn macos_active() -> Result<Option<WindowInfo>> {
    let text = osascript(&[
        "tell application \"System Events\"".to_string(),
        "set frontApp to first application process whose frontmost is true".to_string(),
        "set pidText to (unix id of frontApp) as text".to_string(),
        "set appName to name of frontApp".to_string(),
        "try".to_string(),
        "set focusedWindow to (value of attribute \"AXFocusedWindow\" of frontApp)".to_string(),
        "on error".to_string(),
        format!("return pidText & \"\\n\" & appName & \"\\n\" & \"{NO_FOCUSED_WINDOW}\""),
        "end try".to_string(),
        "set windowName to \"\"".to_string(),
        "try".to_string(),
        "set windowName to (name of focusedWindow) as text".to_string(),
        "end try".to_string(),
        "return pidText & \"\\n\" & appName & \"\\n\" & windowName".to_string(),
        "end tell".to_string(),
    ])?;
    Ok(parse_macos_active(&text))
}

/// Three lines from [`macos_active`]: pid, application, window title.
#[cfg(target_os = "macos")]
pub fn parse_macos_active(text: &str) -> Option<WindowInfo> {
    let mut lines = text.lines();
    let pid: u32 = lines.next()?.trim().parse().ok()?;
    let app = lines.next()?.trim();
    let title = lines.next().unwrap_or("").trim();
    if title == NO_FOCUSED_WINDOW {
        return None;
    }
    Some(WindowInfo {
        title: (!title.is_empty()).then(|| title.to_string()),
        id: None,
        pid: Some(pid),
        app: (!app.is_empty()).then(|| app.to_string()),
        active: Some(true),
        minimized: None,
        maximized: None,
    })
}

#[cfg(target_os = "macos")]
fn macos_focus(window: &WindowInfo) -> Result<()> {
    let pid = macos_pid(window)?;
    match window.title.clone().filter(|title| !title.is_empty()) {
        Some(title) => osascript(&[
            "tell application \"System Events\"".to_string(),
            format!("set target to first application process whose unix id is {pid}"),
            "set frontmost of target to true".to_string(),
            format!(
                "perform action \"AXRaise\" of (first window of target whose name is \"{}\")",
                applescript_escape(&title)
            ),
            "end tell".to_string(),
        ])?,
        // Without a title only the application can be raised; that is the
        // documented limit of the title-less path.
        None => osascript(&[
            "tell application \"System Events\"".to_string(),
            format!("set frontmost of (first application process whose unix id is {pid}) to true"),
            "end tell".to_string(),
        ])?,
    };
    Ok(())
}

#[cfg(target_os = "macos")]
fn macos_minimize(window: &WindowInfo) -> Result<()> {
    let pid = macos_pid(window)?;
    let title = macos_title(window)?;
    osascript(&[
        "tell application \"System Events\"".to_string(),
        format!("set target to first application process whose unix id is {pid}"),
        format!(
            "set value of attribute \"AXMinimized\" of (first window of target whose name is \"{}\") to true",
            applescript_escape(&title)
        ),
        "end tell".to_string(),
    ])?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn macos_maximize(window: &WindowInfo) -> Result<()> {
    let pid = macos_pid(window)?;
    let title = macos_title(window)?;
    osascript(&[
        "tell application \"System Events\"".to_string(),
        format!("set target to first application process whose unix id is {pid}"),
        format!(
            "perform action \"AXZoomWindow\" of (first window of target whose name is \"{}\")",
            applescript_escape(&title)
        ),
        "end tell".to_string(),
    ])?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn macos_pid(window: &WindowInfo) -> Result<u32> {
    window.pid.ok_or_else(|| {
        Error::invalid_input(
            "the macOS adapter addresses windows by their application pid; this row has none — re-run `x window list`",
        )
    })
}

#[cfg(target_os = "macos")]
fn macos_title(window: &WindowInfo) -> Result<String> {
    window
        .title
        .clone()
        .filter(|title| !title.is_empty())
        .ok_or_else(|| {
            Error::invalid_input(
                "address by title is required here, and this row has none; grant Screen Recording so `x window list` reports titles",
            )
        })
}

/// Run `osascript` with one `-e` per script line.
#[cfg(target_os = "macos")]
fn osascript(lines: &[String]) -> Result<String> {
    let mut args: Vec<&str> = Vec::with_capacity(lines.len() * 2);
    for line in lines {
        args.push("-e");
        args.push(line);
    }
    let output = Command::new("osascript")
        .args(&args)
        .output()
        .map_err(|e| Error::system(format!("cannot run osascript: {e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        // The Accessibility refusal has a stable shape; everything else is
        // passed through verbatim.
        if stderr.contains("assistive access")
            || stderr.contains("-1719")
            || stderr.contains("-25211")
        {
            return Err(Error::permission_denied(
                x_core::PermissionRequirement::Elevated,
                format!(
                    "macOS refused UI scripting ({stderr}); grant Accessibility permission to the terminal in System Settings -> Privacy & Security -> Accessibility"
                ),
            ));
        }
        return Err(Error::system(if stderr.is_empty() {
            format!("osascript failed (exit {:?})", output.status.code())
        } else {
            format!("osascript failed: {stderr}")
        }));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Escape a string for an AppleScript double-quoted literal.
#[cfg(target_os = "macos")]
pub fn applescript_escape(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// One integer field of a CG window dictionary.
#[cfg(target_os = "macos")]
fn number(dict: &CFDictionary<CFString, CFType>, key: &'static str) -> Option<i64> {
    dict.find(CFString::from_static_string(key))
        .and_then(|value| value.downcast::<CFNumber>())
        .and_then(|number| number.to_i64())
}

/// One string field of a CG window dictionary; empty reads as absent.
#[cfg(target_os = "macos")]
fn string(dict: &CFDictionary<CFString, CFType>, key: &'static str) -> Option<String> {
    dict.find(CFString::from_static_string(key))
        .and_then(|value| value.downcast::<CFString>())
        .map(|text| text.to_string())
        .filter(|text| !text.is_empty())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    #[test]
    fn wmctrl_lines_split_id_pid_and_the_rest_of_the_title() {
        let text = "0x02a00004  0 12345  my-host Terminal — bash\n\
                    not a window line\n\
                    0x1400007  0 999  my-host  spaced  title\n";
        let rows = super::parse_wmctrl(text);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id.as_deref(), Some("0x02A00004"));
        assert_eq!(rows[0].pid, Some(12345));
        assert_eq!(rows[0].title.as_deref(), Some("Terminal — bash"));
        assert_eq!(rows[1].id.as_deref(), Some("0x01400007"));
        assert_eq!(rows[1].title.as_deref(), Some("spaced  title"));
        assert_eq!(rows[0].active, None, "unknown is not false");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ids_canonicalize_across_wmctrl_and_xprop_padding() {
        assert_eq!(
            super::canonical_id("0x02a00004").as_deref(),
            Some("0x02A00004")
        );
        assert_eq!(
            super::canonical_id("0x2a00004").as_deref(),
            Some("0x02A00004")
        );
        assert_eq!(super::canonical_id("garbage"), None);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_active_window_line_yields_a_canonical_id_or_none() {
        let text = "_NET_ACTIVE_WINDOW(WINDOW): window id # 0x2a00004\n";
        assert_eq!(super::parse_active(text).as_deref(), Some("0x02A00004"));
        assert_eq!(super::parse_active("_NET_ACTIVE_WINDOW:  not found."), None);
        // EWMH's "no window" value.
        assert_eq!(
            super::parse_active("_NET_ACTIVE_WINDOW(WINDOW): window id # 0x0"),
            None
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn state_atoms_parse_from_the_property_line_only() {
        let text =
            "_NET_WM_STATE(ATOM) = _NET_WM_STATE_MAXIMIZED_VERT, _NET_WM_STATE_MAXIMIZED_HORZ\n";
        let atoms = super::parse_state_atoms(text);
        assert_eq!(atoms.len(), 2);
        assert_eq!(
            super::parse_state_atoms("_NET_WM_STATE:  not found."),
            Vec::<String>::new()
        );
        assert!(super::parse_state_atoms("").is_empty());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_active_script_output_maps_pid_app_and_title() {
        let row = super::parse_macos_active("501\nSafari\nStart Page\n").expect("active");
        assert_eq!(row.pid, Some(501));
        assert_eq!(row.app.as_deref(), Some("Safari"));
        assert_eq!(row.title.as_deref(), Some("Start Page"));
        assert_eq!(row.active, Some(true));
        // No focused window is a fact, not an empty row.
        let none = super::parse_macos_active("501\nFinder\n__no_focused_window__\n");
        assert_eq!(none, None);
        // A focused window without a name keeps pid and app.
        let unnamed = super::parse_macos_active("501\nFinder\n\n").expect("active");
        assert_eq!(unnamed.title, None);
        assert_eq!(unnamed.app.as_deref(), Some("Finder"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn applescript_literals_escape_quotes_and_backslashes() {
        assert_eq!(super::applescript_escape(r#"a "b" \c"#), r#"a \"b\" \\c"#);
    }
}
