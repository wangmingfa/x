//! Proxy configuration: environment variables and the system-level proxy.
//!
//! `get` reports both layers. `set` / `clear` touch the *process environment*
//! only — every shell and CI runner understands `HTTP_PROXY`, while the
//! system-wide proxy lives in three different places (Windows registry,
//! macOS `scutil`, GNOME/KDE settings); the system layer is reported
//! read-only together with the command that changes it, instead of x editing
//! OS settings behind the user's back.

use serde::{Deserialize, Serialize};

/// Proxy addresses as the environment spells them (`http://host:port`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct EnvProxy {
    /// `HTTP_PROXY` / `http_proxy`.
    pub http: Option<String>,
    /// `HTTPS_PROXY` / `https_proxy`.
    pub https: Option<String>,
    /// `NO_PROXY` / `no_proxy` (comma separated exceptions).
    pub no_proxy: Option<String>,
}

impl EnvProxy {
    /// Read the current process environment (upper case wins over lower case).
    pub fn from_env() -> Self {
        let read = |upper: &str, lower: &str| {
            std::env::var(upper)
                .or_else(|_| std::env::var(lower))
                .ok()
                .filter(|v| !v.is_empty())
        };
        Self {
            http: read("HTTP_PROXY", "http_proxy"),
            https: read("HTTPS_PROXY", "https_proxy"),
            no_proxy: read("NO_PROXY", "no_proxy"),
        }
    }

    /// Set all three in the current process environment.
    pub fn apply_env(&self) {
        if let Some(v) = &self.http {
            std::env::set_var("HTTP_PROXY", v);
        }
        if let Some(v) = &self.https {
            std::env::set_var("HTTPS_PROXY", v);
        }
        if let Some(v) = &self.no_proxy {
            std::env::set_var("NO_PROXY", v);
        }
    }

    /// Remove all six spellings from the current process environment.
    pub fn clear_env() {
        for name in [
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "NO_PROXY",
            "http_proxy",
            "https_proxy",
            "no_proxy",
        ] {
            std::env::remove_var(name);
        }
    }

    /// Whether nothing at all is configured.
    pub fn is_empty(&self) -> bool {
        self.http.is_none() && self.https.is_none() && self.no_proxy.is_none()
    }
}

/// The OS-level proxy as the platform reports it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SystemProxy {
    /// Whether the OS proxy is switched on.
    pub enabled: bool,
    /// HTTP proxy address, when configured.
    pub http: Option<String>,
    /// HTTPS proxy address, when configured.
    pub https: Option<String>,
    /// Exceptions (comma separated).
    pub exceptions: Option<String>,
    /// The command a user would run to change this (guidance, not an action).
    pub how_to_change: Option<String>,
}

/// Proxy inspection and environment-level changes.
pub trait ProxyManager: Send + Sync {
    /// Environment proxy of the current process.
    fn env(&self) -> EnvProxy {
        EnvProxy::from_env()
    }

    /// OS-level proxy, when the platform exposes one.
    fn system(&self) -> Option<SystemProxy> {
        None
    }
}
