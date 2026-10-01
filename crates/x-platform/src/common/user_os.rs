//! Users and groups, one adapter per target OS.
//!
//! Unix reads `/etc/passwd` and `getent` (NSS covers LDAP etc.); macOS prefers
//! `dscl`; Windows enumerates local accounts through `net user` / `net localgroup`.

use std::process::Command;
use x_core::error::{Error, Result};
use x_core::user::{GroupInfo, UserInfo, UserManager};

/// The platform identity adapter.
pub struct PlatformUser;

impl UserManager for PlatformUser {
    fn current(&self) -> Result<UserInfo> {
        #[cfg(unix)]
        {
            let name = x_platform_sys::current_user_name();
            self.info(&name)
        }
        #[cfg(windows)]
        {
            let name = x_platform_sys::current_user_name()
                .ok_or_else(|| Error::unsupported("cannot determine the current user"))?;
            let mut user = self.info(&name).unwrap_or(UserInfo {
                name: name.clone(),
                ..UserInfo::default()
            });
            user.name = name;
            Ok(user)
        }
        #[cfg(not(any(unix, windows)))]
        return Err(Error::unsupported("user detection is not supported here"));
    }

    fn list(&self) -> Result<Vec<UserInfo>> {
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            // getent talks to NSS; fall back to reading /etc/passwd directly.
            if let Ok(text) = run("getent", &["passwd"]) {
                return Ok(parse_passwd(&text));
            }
            let text = std::fs::read_to_string("/etc/passwd")
                .map_err(|e| Error::unsupported(format!("cannot read /etc/passwd: {e}")))?;
            Ok(parse_passwd(&text))
        }
        #[cfg(target_os = "macos")]
        {
            let text = run("dscl", &[".", "-list", "/Users"])?;
            Ok(text
                .lines()
                .map(str::trim)
                .filter(|name| !name.is_empty() && !name.starts_with('_'))
                .map(|name| UserInfo {
                    name: name.to_string(),
                    ..UserInfo::default()
                })
                .collect())
        }
        #[cfg(windows)]
        {
            let text = run("net", &["user"])?;
            // `net user` prints a banner, a blank line, the names in columns,
            // a blank line and a footer; slice between the two dashed lines.
            let lines: Vec<&str> = text.lines().collect();
            let Some(start) = lines.iter().position(|l| l.starts_with("---")) else {
                return Ok(Vec::new());
            };
            let mut users = Vec::new();
            for line in lines.iter().skip(start + 1) {
                if line.starts_with("---") || line.trim().is_empty() {
                    break;
                }
                for name in line.split_whitespace() {
                    users.push(UserInfo {
                        name: name.to_string(),
                        ..UserInfo::default()
                    });
                }
            }
            Ok(users)
        }
        #[cfg(not(any(unix, windows)))]
        return Err(Error::unsupported("user listing is not supported here"));
    }

    fn groups(&self) -> Result<Vec<GroupInfo>> {
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            if let Ok(text) = run("getent", &["group"]) {
                return Ok(parse_group(&text));
            }
            let text = std::fs::read_to_string("/etc/group")
                .map_err(|e| Error::unsupported(format!("cannot read /etc/group: {e}")))?;
            Ok(parse_group(&text))
        }
        #[cfg(target_os = "macos")]
        {
            let text = run("dscl", &[".", "-list", "/Groups"])?;
            Ok(text
                .lines()
                .map(str::trim)
                .filter(|name| !name.is_empty() && !name.starts_with('_'))
                .map(|name| GroupInfo {
                    name: name.to_string(),
                    ..GroupInfo::default()
                })
                .collect())
        }
        #[cfg(windows)]
        {
            let text = run("net", &["localgroup"])?;
            let lines: Vec<&str> = text.lines().collect();
            let Some(start) = lines.iter().position(|l| l.starts_with("---")) else {
                return Ok(Vec::new());
            };
            let mut groups = Vec::new();
            for line in lines.iter().skip(start + 1) {
                if line.starts_with("---") || line.trim().is_empty() {
                    break;
                }
                for name in line.split_whitespace() {
                    groups.push(GroupInfo {
                        name: name.to_string(),
                        ..GroupInfo::default()
                    });
                }
            }
            Ok(groups)
        }
        #[cfg(not(any(unix, windows)))]
        return Err(Error::unsupported("group listing is not supported here"));
    }

    fn info(&self, name: &str) -> Result<UserInfo> {
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            if let Ok(text) = run("getent", &["passwd", name]) {
                if let Some(user) = parse_passwd(&text).into_iter().next() {
                    return Ok(with_groups(user));
                }
            }
            Err(Error::not_found(format!("no such user: {name}")))
        }
        #[cfg(target_os = "macos")]
        {
            let text = run("dscl", &[".", "-read", &format!("/Users/{name}")])?;
            Ok(dscl_user(&text, name))
        }
        #[cfg(windows)]
        {
            let text = run("net", &["user", name])?;
            Ok(net_user(&text, name))
        }
        #[cfg(not(any(unix, windows)))]
        return Err(Error::unsupported("user info is not supported here"));
    }

    fn group(&self, name: &str) -> Result<GroupInfo> {
        #[cfg(all(unix, not(target_os = "macos")))]
        {
            if let Ok(text) = run("getent", &["group", name]) {
                if let Some(group) = parse_group(&text).into_iter().next() {
                    return Ok(group);
                }
            }
            Err(Error::not_found(format!("no such group: {name}")))
        }
        #[cfg(target_os = "macos")]
        {
            let text = run("dscl", &[".", "-read", &format!("/Groups/{name}")])?;
            Ok(dscl_group(&text, name))
        }
        #[cfg(windows)]
        {
            let text = run("net", &["localgroup", name])?;
            Ok(net_group(&text, name))
        }
        #[cfg(not(any(unix, windows)))]
        return Err(Error::unsupported("group info is not supported here"));
    }
}

/// User groups via `id -nG` when available; harmless when it is not.
#[cfg(all(unix, not(target_os = "macos")))]
fn with_groups(mut user: UserInfo) -> UserInfo {
    if let Ok(text) = run("id", &["-nG", &user.name]) {
        user.groups = text.split_whitespace().map(str::to_string).collect();
    }
    user
}

#[cfg(all(unix, not(target_os = "macos")))]
fn parse_passwd(text: &str) -> Vec<UserInfo> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let mut fields = line.split(':');
            Some(UserInfo {
                name: fields.next()?.to_string(),
                uid: fields.next().map(str::to_string),
                gid: fields.next().map(str::to_string),
                full_name: None,
                home: fields.nth(2).map(str::to_string),
                shell: fields.next().map(str::to_string),
                groups: Vec::new(),
            })
        })
        .collect()
}

#[cfg(all(unix, not(target_os = "macos")))]
fn parse_group(text: &str) -> Vec<GroupInfo> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            let mut fields = line.split(':');
            let name = fields.next()?.to_string();
            let gid = fields.next().map(str::to_string);
            let members = fields
                .last()
                .map(|m| {
                    m.split(',')
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            Some(GroupInfo { name, gid, members })
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn dscl_user(text: &str, name: &str) -> UserInfo {
    let mut user = UserInfo {
        name: name.to_string(),
        ..UserInfo::default()
    };
    for line in text.lines() {
        let (key, value) = line.split_once(": ").unwrap_or((line, ""));
        match key {
            "UniqueID" => user.uid = Some(value.trim().to_string()),
            "PrimaryGroupID" => user.gid = Some(value.trim().to_string()),
            "RealName" => user.full_name = Some(value.trim().to_string()),
            "NFSHomeDirectory" => user.home = Some(value.trim().to_string()),
            "UserShell" => user.shell = Some(value.trim().to_string()),
            _ => {}
        }
    }
    user
}

#[cfg(target_os = "macos")]
fn dscl_group(text: &str, name: &str) -> GroupInfo {
    let mut group = GroupInfo {
        name: name.to_string(),
        ..GroupInfo::default()
    };
    for line in text.lines() {
        let (key, value) = line.split_once(": ").unwrap_or((line, ""));
        match key {
            "PrimaryGroupID" => group.gid = Some(value.trim().to_string()),
            "GroupMembership" => {
                group.members = value.split_whitespace().map(str::to_string).collect()
            }
            _ => {}
        }
    }
    group
}

#[cfg(windows)]
fn net_user(text: &str, name: &str) -> UserInfo {
    let mut user = UserInfo {
        name: name.to_string(),
        ..UserInfo::default()
    };
    for line in text.lines() {
        let (key, value) = line
            .split_once("  ")
            .map(|(k, v)| (k.trim(), v.trim()))
            .unwrap_or((line.trim(), ""));
        match key {
            "Full Name" => user.full_name = Some(value.to_string()),
            "User's home directory" | "Home Directory" => user.home = Some(value.to_string()),
            _ => {}
        }
    }
    user
}

#[cfg(windows)]
fn net_group(text: &str, name: &str) -> GroupInfo {
    let mut group = GroupInfo {
        name: name.to_string(),
        ..GroupInfo::default()
    };
    let mut members_started = false;
    for line in text.lines() {
        if members_started {
            if line.starts_with("---") || line.trim().starts_with("The command completed") {
                break;
            }
            group
                .members
                .extend(line.split_whitespace().map(str::to_string));
            continue;
        }
        if line.trim() == "Members" {
            members_started = true;
        }
    }
    group
}

fn run(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| Error::unsupported(format!("{program} is not available: {e}")))?;
    if !output.status.success() {
        return Err(Error::not_found(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Re-export of the sys helper so the cfg arms above compile in one place.
use crate::sys as x_platform_sys;
