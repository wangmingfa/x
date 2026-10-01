//! `x permission`: who may touch a path, and what the OS says about it.

use clap::Subcommand;
use x_core::error::{Error, Result};
use x_core::SystemContext;
use x_platform::common::pathperm_os::has_executable_bit;

use crate::{
    format::{OutputFormat, Renderer, Table},
    row,
};

/// `x permission` subcommands.
#[derive(Debug, Subcommand)]
pub enum PermissionCommand {
    /// Access probe plus ownership / mode / ACL detail for one path.
    Info {
        /// Path to inspect.
        path: String,
    },

    /// Exit 0 when the action is allowed, 1 otherwise.
    Check {
        /// Action to probe: `read`, `write` or `execute`.
        action: String,
        /// Path to probe.
        path: String,
    },
}

/// Arguments for `x permission`.
#[derive(Debug, clap::Args)]
pub struct PermissionArgs {
    #[command(subcommand)]
    pub command: PermissionCommand,
}

/// Route a `x permission` invocation.
pub fn dispatch(
    context: &SystemContext,
    renderer: &mut Renderer,
    command: &PermissionCommand,
) -> Result<i32> {
    match command {
        PermissionCommand::Info { path } => info(context, renderer, path),
        PermissionCommand::Check { action, path } => check(renderer, action, path),
    }
}

fn info(context: &SystemContext, renderer: &mut Renderer, path: &str) -> Result<i32> {
    let access = x_core::pathperm::check(path.as_ref(), has_executable_bit);
    let file = context.file.as_ref();

    if renderer.format() == OutputFormat::Json {
        let detail = file.and_then(|f| f.acl(path.as_ref()).ok().flatten());
        renderer.always_json(&serde_json::json!({
            "path": path,
            "access": access,
            "acl": detail,
        }))?;
        return Ok(0);
    }

    let mut table = Table::new(["field", "value"]);
    table.push(row!["path", path]);
    if !access.exists {
        table.push(row!["exists", "false"]);
        renderer.table(&table)?;
        return Ok(0);
    }
    table.push(row!["readable", access.readable.to_string()]);
    table.push(row!["writable", access.writable.to_string()]);
    table.push(row!["executable", access.executable.to_string()]);

    if let Some(file) = file {
        if let Ok(info) = file.info(path.as_ref()) {
            if let Some(mode) = &info.mode {
                table.push(row!["mode", mode.clone()]);
            }
            if let Some(owner) = &info.owner {
                table.push(row!["owner", owner.clone()]);
            }
            if let Some(group) = &info.group {
                table.push(row!["group", group.clone()]);
            }
            table.push(row!["readonly", info.readonly.to_string()]);
        }
        if let Ok(Some(acl)) = file.acl(path.as_ref()) {
            table.push(row!["acl", acl.replace('\n', " | ")]);
        }
    }
    renderer.table(&table)?;
    Ok(0)
}

fn check(renderer: &mut Renderer, action: &str, path: &str) -> Result<i32> {
    let access = x_core::pathperm::check(path.as_ref(), has_executable_bit);
    let allowed = match action {
        "read" => access.exists && access.readable,
        "write" => access.exists && access.writable,
        "execute" => access.exists && access.executable,
        other => {
            return Err(Error::invalid_input(format!(
                "unknown action `{other}`; expected read, write or execute"
            )))
        }
    };
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(&serde_json::json!({
            "path": path,
            "action": action,
            "allowed": allowed,
        }))?;
        return Ok(if allowed { 0 } else { 1 });
    }
    renderer.line(format!(
        "{action} on {path}: {}",
        if allowed { "allowed" } else { "denied" }
    ))?;
    Ok(if allowed { 0 } else { 1 })
}
