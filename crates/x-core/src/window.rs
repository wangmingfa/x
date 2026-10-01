//! Open windows: one read over the desktop the platform composes, plus the
//! three verbs that move a window (focus, minimize, maximize).
//!
//! Windows walks the top-level windows natively (`EnumWindows` and friends);
//! Linux drives `wmctrl` / `xprop`; macOS reads the CG window list for the
//! inventory and drives System Events through `osascript` for the verbs.
//! Whatever a platform withholds — window titles on macOS without the Screen
//! Recording permission, focus facts on a bare X session — stays `None`.
//!
//! Reads are permission-free where the platform allows it; the verbs are
//! state changes and default to [`crate::error::ErrorKind::Unsupported`] so
//! only a backend with a real path overrides them. Frontends confirm and
//! audit the verbs; this module never runs anything.

use serde::{Deserialize, Serialize};

/// One top-level window as the platform sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WindowInfo {
    /// Window title, when the platform reports one. macOS withholds titles
    /// from processes that lack the Screen Recording permission.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Platform identifier used to address the window (`0x…` HWND / X11 id).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Owning process id, when the platform reports it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    /// Owning application name (executable stem, comm name, bundle owner).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// Holds the input focus, when the platform tells us.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
    /// Iconified / minimized.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minimized: Option<bool>,
    /// Maximized / zoomed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maximized: Option<bool>,
}

impl WindowInfo {
    /// A window whose title is its only known fact so far.
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: Some(title.into()),
            id: None,
            pid: None,
            app: None,
            active: None,
            minimized: None,
            maximized: None,
        }
    }

    /// How a frontend names this window in messages and audit records.
    pub fn label(&self) -> String {
        match (&self.title, &self.id) {
            (Some(title), _) if !title.is_empty() => format!("`{title}`"),
            (_, Some(id)) => format!("window {id}"),
            _ => "an untitled window".to_string(),
        }
    }
}

/// Window inventory and control.
pub trait WindowManager: Send + Sync {
    /// Every manageable top-level window, in platform order.
    fn windows(&self) -> crate::error::Result<Vec<WindowInfo>>;

    /// The window that currently holds the input focus; `None` when the
    /// platform is sure no window is focused (bare X session, empty desktop).
    fn active(&self) -> crate::error::Result<Option<WindowInfo>>;

    /// Raise and focus `window`. State change: confirm and audit.
    fn focus(&self, _window: &WindowInfo) -> crate::error::Result<()> {
        Err(crate::Error::unsupported(
            "this platform has no non-interactive window focus",
        ))
    }

    /// Minimize (iconify) `window`. State change: confirm and audit.
    fn minimize(&self, _window: &WindowInfo) -> crate::error::Result<()> {
        Err(crate::Error::unsupported(
            "this platform has no non-interactive window minimize",
        ))
    }

    /// Maximize (zoom) `window`. State change: confirm and audit.
    fn maximize(&self, _window: &WindowInfo) -> crate::error::Result<()> {
        Err(crate::Error::unsupported(
            "this platform has no non-interactive window maximize",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_serialize_only_what_is_known() {
        let mut window = WindowInfo::new("editor — main.rs");
        window.pid = Some(4242);
        let json = serde_json::to_string(&window).expect("json");
        assert_eq!(json, "{\"title\":\"editor — main.rs\",\"pid\":4242}");
    }

    #[test]
    fn round_trip_keeps_the_states() {
        let mut window = WindowInfo::new("Browser");
        window.id = Some("0x00012345".into());
        window.active = Some(true);
        window.minimized = Some(false);
        window.maximized = Some(false);
        let back: WindowInfo =
            serde_json::from_str(&serde_json::to_string(&window).expect("json")).expect("back");
        assert_eq!(back, window);
    }

    #[test]
    fn label_prefers_the_title_then_the_id() {
        assert_eq!(WindowInfo::new("Editor").label(), "`Editor`");
        let mut id_only = WindowInfo {
            title: None,
            id: Some("0x0042".into()),
            pid: None,
            app: None,
            active: None,
            minimized: None,
            maximized: None,
        };
        assert_eq!(id_only.label(), "window 0x0042");
        id_only.id = None;
        assert_eq!(id_only.label(), "an untitled window");
    }

    #[test]
    fn verbs_without_a_backend_are_unsupported() {
        struct Bare;
        impl WindowManager for Bare {
            fn windows(&self) -> crate::error::Result<Vec<WindowInfo>> {
                Ok(Vec::new())
            }
            fn active(&self) -> crate::error::Result<Option<WindowInfo>> {
                Ok(None)
            }
        }

        let window = WindowInfo::new("x");
        for result in [
            Bare.focus(&window),
            Bare.minimize(&window),
            Bare.maximize(&window),
        ] {
            let err = result.expect_err("default verbs must refuse");
            assert_eq!(err.kind(), crate::error::ErrorKind::Unsupported);
        }
    }
}
