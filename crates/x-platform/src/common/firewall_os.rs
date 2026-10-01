//! Firewall adapter: status and rules through each OS's own tool.
//!
//! Read paths: `netsh advfirewall` on Windows, `nft`/`iptables`/`ufw` on
//! Linux (first one present wins), `pfctl` on macOS. Writes (`allow` /
//! `deny`) need elevation everywhere and are confirmed by the CLI; the
//! adapters attach the structured permission requirement on failure.

use std::process::Command;
use x_core::error::{Error, Result};
use x_core::firewall::{FirewallManager, FirewallRule, FirewallStack};

/// The platform firewall adapter.
pub struct PlatformFirewall;

impl FirewallManager for PlatformFirewall {
    fn stack(&self) -> FirewallStack {
        #[cfg(windows)]
        return FirewallStack::WindowsFirewall;
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            for tool in ["nft", "iptables", "ufw"] {
                if x_core::devenv::which(tool).is_some() {
                    return FirewallStack::Netfilter;
                }
            }
            FirewallStack::Unknown
        }
        #[cfg(target_os = "macos")]
        return FirewallStack::Pf;
        #[cfg(not(any(unix, windows)))]
        return FirewallStack::Unknown;
    }

    fn enabled(&self) -> Result<bool> {
        #[cfg(windows)]
        {
            let output = run_netsh(&["advfirewall", "show", "allprofiles", "state"])?;
            Ok(output.lines().any(|line| {
                let line = line.trim();
                let is_state =
                    line.starts_with("状态") || line.to_ascii_lowercase().starts_with("state");
                is_state && (line.contains("ON") || line.contains("启用"))
            }))
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            for tool in ["ufw", "nft", "iptables"] {
                if x_core::devenv::which(tool).is_none() {
                    continue;
                }
                let output = match tool {
                    "ufw" => run_tool("ufw", &["status"], true),
                    "nft" => run_tool("nft", &["list", "ruleset"], true),
                    _ => run_tool("iptables", &["-L", "-n"], true),
                };
                match output {
                    Ok(text) => {
                        let lowered = text.to_ascii_lowercase();
                        if tool == "ufw" {
                            return Ok(lowered.contains("status: active"));
                        }
                        return Ok(!lowered.trim().is_empty());
                    }
                    Err(_) => continue,
                }
            }
            Ok(false)
        }
        #[cfg(target_os = "macos")]
        {
            let output = run_tool(
                "/usr/libexec/ApplicationFirewall/socketfilterfw",
                &["--getglobalstate"],
                false,
            )?;
            Ok(output.to_ascii_lowercase().contains("enabled"))
        }
        #[cfg(not(any(unix, windows)))]
        return Err(Error::unsupported("firewall status is not supported here"));
    }

    fn list(&self) -> Result<Vec<FirewallRule>> {
        #[cfg(windows)]
        {
            windows_rules()
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            if x_core::devenv::which("ufw").is_some() {
                return ufw_rules();
            }
            Ok(Vec::new())
        }
        #[cfg(target_os = "macos")]
        {
            // pf's rule listing needs root and anchoring; Application Firewall
            // is what users actually toggle, so report its blocked apps.
            let output = run_tool(
                "/usr/libexec/ApplicationFirewall/socketfilterfw",
                &["--listapps"],
                false,
            )
            .unwrap_or_default();
            let mut rules = Vec::new();
            for line in output.lines() {
                let line = line.trim();
                if let Some(path) = line.strip_prefix('(').and_then(|l| l.strip_suffix(')')) {
                    rules.push(FirewallRule {
                        name: path.to_string(),
                        action: "deny-unless-allowed".to_string(),
                        port: None,
                        protocol: None,
                        enabled: true,
                    });
                }
            }
            Ok(rules)
        }
        #[cfg(not(any(unix, windows)))]
        return Err(Error::unsupported("firewall listing is not supported here"));
    }

    fn allow(&self, port: u16, protocol: Option<&str>, name: Option<&str>) -> Result<()> {
        modify(port, protocol, name, true)
    }

    fn deny(&self, port: u16, protocol: Option<&str>, name: Option<&str>) -> Result<()> {
        modify(port, protocol, name, false)
    }
}

/// Platform write path for `allow` / `deny`.
fn modify(port: u16, protocol: Option<&str>, name: Option<&str>, allow: bool) -> Result<()> {
    let proto = protocol.unwrap_or("tcp");
    #[cfg(windows)]
    {
        let verb = if allow { "allow" } else { "deny" };
        let default_name = format!("x-{verb}-{port}-{proto}");
        let rule_name = name.unwrap_or(&default_name);
        let action = if allow { "Allow" } else { "Block" };
        let output = Command::new("netsh")
            .args([
                "advfirewall",
                "firewall",
                "add",
                "rule",
                &format!("name={rule_name}"),
                "dir=In",
                &format!("action={action}"),
                &format!("protocol={proto}"),
                &format!("localport={port}"),
            ])
            .output()
            .map_err(|e| Error::system(format!("cannot run netsh: {e}")))?;
        finish(output, x_core::PermissionRequirement::Administrator)
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if x_core::devenv::which("ufw").is_some() {
            let action = if allow { "allow" } else { "deny" };
            let mut command = Command::new("ufw");
            command.arg(action).arg(format!("{port}/{proto}"));
            if let Some(name) = name {
                // ufw stores named rules as comments on the rule line.
                command.args(["comment", name]);
            }
            let output = command
                .output()
                .map_err(|e| Error::system(format!("cannot run ufw: {e}")))?;
            return finish(output, x_core::PermissionRequirement::Root);
        }
        Err(Error::unsupported(
            "no supported firewall tool found (install ufw, or manage nftables/iptables yourself)",
        ))
    }
    #[cfg(target_os = "macos")]
    {
        // pf anchor edits need a pf.conf change and root; report honestly.
        let _ = (port, proto, name, allow);
        Err(Error::unsupported(
            "macOS pf rule changes need a pf anchor and root; use the Application Firewall UI or pfctl manually",
        ))
    }
    #[cfg(not(any(unix, windows)))]
    return Err(Error::unsupported(
        "firewall modification is not supported here",
    ));
}

#[cfg(not(target_os = "macos"))]
fn finish(output: std::process::Output, requirement: x_core::PermissionRequirement) -> Result<()> {
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let mut error = Error::system(format!(
        "firewall change failed: {}",
        if stderr.is_empty() {
            "non-zero exit"
        } else {
            &stderr
        }
    ));
    error = error.with_permission(requirement);
    Err(error)
}

#[cfg(windows)]
fn run_netsh(args: &[&str]) -> Result<String> {
    let (code, stdout, stderr) = crate::sys::run_command_capture("netsh", args)
        .map_err(|e| Error::system(format!("cannot run netsh: {e}")))?;
    if code == 0 {
        return Ok(crate::windows::service::decode_console(&stdout));
    }
    let stderr = crate::windows::service::decode_console(&stderr);
    let stderr = stderr.trim();
    Err(Error::system(if stderr.is_empty() {
        "netsh failed".to_string()
    } else {
        format!("netsh failed: {stderr}")
    }))
}

/// Run a tool that needs root on Linux, or not on macOS; stderr allowed.
#[cfg(not(windows))]
fn run_tool(program: &str, args: &[&str], needs_root: bool) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| Error::system(format!("cannot run {program}: {e}")))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let mut error = Error::system(format!(
        "{program} failed: {}",
        if stderr.is_empty() {
            "non-zero exit"
        } else {
            &stderr
        }
    ));
    if needs_root {
        error = error.with_permission(x_core::PermissionRequirement::Root);
    }
    Err(error)
}

#[cfg(windows)]
fn windows_rules() -> Result<Vec<FirewallRule>> {
    let text = run_netsh(&["advfirewall", "firewall", "show", "rule", "name=all"])?;
    Ok(parse_windows_rules(&text))
}

/// Parse a `netsh advfirewall firewall show rule name=all` dump.
///
/// zh-CN Windows ships a fully translated dump, so both the English and the
/// Chinese key names are matched, and the action / enabled values are
/// normalised to the model's lowercase verbs.
#[cfg(windows)]
fn parse_windows_rules(text: &str) -> Vec<FirewallRule> {
    let mut rules = Vec::new();
    let mut current: Option<FirewallRule> = None;
    for line in text.lines() {
        let Some((key, value)) = line.trim().split_once(':') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if key == "Rule Name" || key == "规则名称" {
            if let Some(rule) = current.take() {
                if !rule.name.is_empty() {
                    rules.push(rule);
                }
            }
            current = Some(FirewallRule {
                name: value.to_string(),
                enabled: true,
                ..FirewallRule::default()
            });
            continue;
        }
        let Some(rule) = current.as_mut() else {
            continue;
        };
        match key {
            "Action" | "操作" => rule.action = normalize_action(value),
            "LocalPort" | "本地端口" => {
                if !value.is_empty() && value != "Any" && value != "任何" {
                    rule.port = Some(value.to_string());
                }
            }
            "Protocol" | "协议" => {
                if !value.is_empty() && value != "Any" && value != "任何" {
                    rule.protocol = Some(value.to_ascii_lowercase());
                }
            }
            "Enabled" | "已启用" => rule.enabled = value == "Yes" || value == "是",
            _ => {}
        }
    }
    if let Some(rule) = current.take() {
        if !rule.name.is_empty() {
            rules.push(rule);
        }
    }
    rules
}

#[cfg(windows)]
fn normalize_action(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "allow" | "允许" => "allow",
        "block" | "阻止" => "block",
        "bypass" | "旁路" => "bypass",
        "reject" | "拒绝" => "reject",
        _ => return value.trim().to_ascii_lowercase(),
    }
    .to_string()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn ufw_rules() -> Result<Vec<FirewallRule>> {
    let text = run_tool("ufw", &["status", "numbered"], true)?;
    Ok(parse_ufw_rules(&text))
}

/// Parse `ufw status numbered` (or plain `ufw status`) output.
///
/// Numbered rules start with `[ 3] `, which shifts the interesting fields by
/// two tokens; comment continuations like `Anywhere on 22/tcp` are skipped.
#[cfg(all(unix, not(target_os = "macos")))]
fn parse_ufw_rules(text: &str) -> Vec<FirewallRule> {
    let mut rules = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("Status") || line.starts_with("To") {
            continue;
        }
        let mut fields = line.split_whitespace();
        if fields.clone().next() == Some("[") {
            fields.next();
            fields.next();
        }
        let Some(first) = fields.next() else {
            continue;
        };
        // Rows start with the to-address: `22/tcp`, `8080`, or `Anywhere on
        // 53/tcp` prose that still names the port one token later.
        let (to, action) = if first == "Anywhere" {
            if fields.next() == Some("on") {
                let Some(port) = fields.next() else { continue };
                let Some(action) = fields.next() else {
                    continue;
                };
                (port, action)
            } else {
                continue;
            }
        } else {
            let Some(action) = fields.next() else {
                continue;
            };
            if !action.starts_with(|c: char| c.is_ascii_alphabetic()) {
                continue;
            }
            (first, action)
        };
        if !to.contains('/')
            && !to
                .bytes()
                .all(|b| b.is_ascii_digit() || b == b'-' || b == b':')
        {
            continue;
        }
        let (port, protocol) = match to.split_once('/') {
            Some((p, proto)) => (Some(p.to_string()), Some(proto.to_lowercase())),
            None => (Some(to.to_string()), None),
        };
        rules.push(FirewallRule {
            name: format!("ufw {to}"),
            action: action.to_ascii_lowercase(),
            port,
            protocol,
            enabled: true,
        });
    }
    rules
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn english_netsh_dumps_are_parsed_field_by_field() {
        let text = "Rule Name:                            x-allow-8080-tcp\n\
                     Description:                          opened by x\n\
                     Protocol:                             TCP\n\
                     LocalPort:                            8080\n\
                     RemotePort:                           Any\n\
                     Action:                               Allow\n\
                     Enabled:                              Yes\n\
                     \n\
                     Rule Name:                            some-block\n\
                     Description:\n\
                     Protocol:                             UDP\n\
                     LocalPort:                            Any\n\
                     Action:                               Block\n\
                     Enabled:                              No\n";
        let rules = parse_windows_rules(text);
        assert_eq!(rules.len(), 2, "{rules:?}");
        assert_eq!(rules[0].name, "x-allow-8080-tcp");
        assert_eq!(rules[0].action, "allow");
        assert_eq!(rules[0].port.as_deref(), Some("8080"));
        assert_eq!(rules[0].protocol.as_deref(), Some("tcp"));
        assert!(rules[0].enabled);
        assert_eq!(rules[1].action, "block");
        assert_eq!(rules[1].port, None);
        assert!(!rules[1].enabled);
    }

    #[cfg(windows)]
    #[test]
    fn chinese_netsh_dumps_are_normalized() {
        let text = "规则名称:                          x-deny-9090-tcp\n\
                     说明:\n\
                     协议:                            TCP\n\
                     本地端口:                        9090\n\
                     远程端口:                        任何\n\
                     操作:                            阻止\n\
                     已启用:                          否\n";
        let rules = parse_windows_rules(text);
        assert_eq!(rules.len(), 1, "{rules:?}");
        assert_eq!(rules[0].name, "x-deny-9090-tcp");
        assert_eq!(rules[0].action, "block");
        assert_eq!(rules[0].port.as_deref(), Some("9090"));
        assert!(!rules[0].enabled);
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn numbered_ufw_output_shifts_the_fields() {
        let text = "Status: active\n\n\
                    To                         Action      From\n\
                    --                         ------      ----\n\
                    [ 1] 22/tcp                     ALLOW IN    Anywhere\n\
                    [ 2] 8080                       DENY IN     10.0.0.0/8\n\
                    [ 3] Anywhere on 53/tcp         ALLOW       Anywhere (v6)\n";
        let rules = parse_ufw_rules(text);
        assert_eq!(rules.len(), 3, "{rules:?}");
        assert_eq!(rules[0].port.as_deref(), Some("22/tcp"));
        assert_eq!(rules[0].protocol.as_deref(), Some("tcp"));
        assert_eq!(rules[0].action, "allow");
        assert_eq!(rules[1].action, "deny");
        assert_eq!(rules[1].protocol, None);
        assert_eq!(rules[2].port.as_deref(), Some("53/tcp"));
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn ufw_header_and_prose_lines_are_not_rules() {
        let rules = parse_ufw_rules("Reading /etc/ufw/user.rules\nUpdating /etc/ufw/user.rules\n");
        assert!(rules.is_empty(), "{rules:?}");
    }

    #[cfg(not(windows))]
    #[test]
    fn the_stack_enum_has_a_stable_name() {
        assert_eq!(FirewallStack::Unknown.name(), "unknown");
        assert_eq!(FirewallStack::Netfilter.name(), "netfilter");
    }
}
