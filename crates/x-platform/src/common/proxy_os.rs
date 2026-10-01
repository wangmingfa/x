//! System-level proxy detection.
//!
//! Read-only on purpose: the OS proxy lives in three different settings
//! stores, and each of them has its own UI / CLI. The report includes the
//! command that changes it so the user stays in control.

use x_core::proxy::SystemProxy;

/// Report the OS proxy configuration, `None` when the platform exposes none.
pub fn system_proxy() -> Option<SystemProxy> {
    #[cfg(windows)]
    {
        windows_proxy()
    }
    #[cfg(target_os = "macos")]
    {
        macos_proxy()
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        linux_proxy()
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

#[cfg(windows)]
fn windows_proxy() -> Option<SystemProxy> {
    // WinInet per-user settings under Internet Settings; `reg query` avoids a
    // winapi dependency in this adapter.
    let output = std::process::Command::new("reg")
        .args([
            "query",
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings",
        ])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let value = |name: &str| -> Option<String> {
        text.lines()
            .find(|line| line.trim_start().starts_with(name))
            .and_then(|line| line.split_whitespace().last())
            .map(|v| v.trim_start_matches("0x").to_string())
            .filter(|v| !v.is_empty() && *v != "0")
    };
    let enabled = value("ProxyEnable").is_some();
    let server = text
        .lines()
        .find(|line| line.trim_start().starts_with("ProxyServer"))
        .and_then(|line| line.split_once("REG_SZ"))
        .map(|(_, v)| v.trim().to_string())
        .filter(|v| !v.is_empty());
    let overrides = text
        .lines()
        .find(|line| line.trim_start().starts_with("ProxyOverride"))
        .and_then(|line| line.split_once("REG_SZ"))
        .map(|(_, v)| v.trim().to_string())
        .filter(|v| !v.is_empty());
    // `server` is `host:port` or `http=…;https=…`.
    let (http, https) = match &server {
        Some(s) if s.contains('=') => (
            s.split(';')
                .find_map(|p| p.strip_prefix("http=").map(str::to_string)),
            s.split(';')
                .find_map(|p| p.strip_prefix("https=").map(str::to_string)),
        ),
        Some(s) => (Some(s.clone()), Some(s.clone())),
        None => (None, None),
    };
    Some(SystemProxy {
        enabled,
        http,
        https,
        exceptions: overrides,
        how_to_change: Some(
            "Settings > Network & Internet > Proxy (or `netsh winhttp set proxy …`)".to_string(),
        ),
    })
}

#[cfg(target_os = "macos")]
fn macos_proxy() -> Option<SystemProxy> {
    let output = std::process::Command::new("scutil")
        .arg("--proxy")
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let value = |key: &str| -> Option<i64> {
        text.lines()
            .find(|line| line.trim_start().starts_with(key))
            .and_then(|line| line.split(':').nth(1))
            .and_then(|v| v.trim().parse().ok())
    };
    let string = |key: &str| -> Option<String> {
        text.lines()
            .find(|line| line.trim_start().starts_with(key))
            .and_then(|line| line.split(':').nth(1))
            .map(|v| v.trim().trim_matches(';').to_string())
            .filter(|v| !v.is_empty())
    };
    let enabled = value("HTTPEnable").unwrap_or(0) == 1 || value("HTTPSEnable").unwrap_or(0) == 1;
    let host = string("HTTPProxy").or_else(|| string("HTTPSProxy"));
    let port = value("HTTPPort").or_else(|| value("HTTPSPort"));
    let address = host.map(|h| match port {
        Some(p) => format!("{h}:{p}"),
        None => h,
    });
    Some(SystemProxy {
        enabled,
        http: address.clone(),
        https: address,
        exceptions: string("ExceptionsList"),
        how_to_change: Some("System Settings > Network > Proxies".to_string()),
    })
}

#[cfg(all(unix, not(target_os = "macos")))]
fn linux_proxy() -> Option<SystemProxy> {
    // GNOME first (`gsettings`), then KDE's kioslave config via env only.
    let gsettings = |schema: &str, key: &str| -> Option<String> {
        std::process::Command::new("gsettings")
            .args(["get", schema, key])
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .trim()
                    .trim_matches('\'')
                    .to_string()
            })
            .filter(|v| !v.is_empty())
    };
    let mode = gsettings("org.gnome.system.proxy", "mode")?;
    let enabled = mode == "manual";
    let host = gsettings("org.gnome.system.proxy.http", "host");
    let port = gsettings("org.gnome.system.proxy.http", "port");
    let address = match (host, port) {
        (Some(h), Some(p)) => Some(format!("{h}:{p}")),
        (Some(h), None) => Some(h),
        _ => None,
    };
    Some(SystemProxy {
        enabled,
        http: address.clone(),
        https: address,
        exceptions: gsettings("org.gnome.system.proxy", "ignore-hosts"),
        how_to_change: Some(
            "gsettings set org.gnome.system.proxy mode 'manual' (or your desktop's proxy settings)"
                .to_string(),
        ),
    })
}
