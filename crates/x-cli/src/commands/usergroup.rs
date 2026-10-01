//! `x user` / `x group`: accounts and memberships.

use clap::Subcommand;
use x_core::error::{Error, Result};
use x_core::SystemContext;

use crate::{
    format::{OutputFormat, Renderer, Table},
    row,
};

/// `x user` subcommands.
#[derive(Debug, Subcommand)]
pub enum UserCommand {
    /// The account this process runs as.
    Current,

    /// Every account the platform exposes.
    List,

    /// One account in detail.
    Info {
        /// User name.
        name: String,
    },
}

/// Arguments for `x user`.
#[derive(Debug, clap::Args)]
pub struct UserArgs {
    #[command(subcommand)]
    pub command: UserCommand,
}

/// Route a `x user` invocation.
pub fn dispatch_user(
    context: &SystemContext,
    renderer: &mut Renderer,
    command: &UserCommand,
) -> Result<i32> {
    let user = context
        .user
        .as_ref()
        .ok_or_else(|| Error::unsupported("user inspection is not available in this context"))?;

    match command {
        UserCommand::Current => {
            render_user(renderer, &user.current()?)?;
        }
        UserCommand::Info { name } => {
            render_user(renderer, &user.info(name)?)?;
        }
        UserCommand::List => {
            let users = user.list()?;
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&users)?;
                return Ok(0);
            }
            let mut table = Table::new(["name", "uid", "home", "shell"]);
            for info in &users {
                table.push(row![
                    info.name.clone(),
                    info.uid.clone().unwrap_or_else(|| "-".into()),
                    info.home.clone().unwrap_or_else(|| "-".into()),
                    info.shell.clone().unwrap_or_else(|| "-".into()),
                ]);
            }
            renderer.table(&table)?;
        }
    }
    Ok(0)
}

fn render_user(renderer: &mut Renderer, info: &x_core::user::UserInfo) -> Result<()> {
    if renderer.format() == OutputFormat::Json {
        renderer.always_json(info)?;
        return Ok(());
    }
    let mut table = Table::new(["field", "value"]);
    table.push(row!["name", info.name.clone()]);
    if let Some(uid) = &info.uid {
        table.push(row!["uid", uid.clone()]);
    }
    if let Some(gid) = &info.gid {
        table.push(row!["gid", gid.clone()]);
    }
    if let Some(full) = &info.full_name {
        table.push(row!["full_name", full.clone()]);
    }
    if let Some(home) = &info.home {
        table.push(row!["home", home.clone()]);
    }
    if let Some(shell) = &info.shell {
        table.push(row!["shell", shell.clone()]);
    }
    if !info.groups.is_empty() {
        table.push(row!["groups", info.groups.join(", ")]);
    }
    renderer.table(&table)?;
    Ok(())
}

/// `x group` subcommands.
#[derive(Debug, Subcommand)]
pub enum GroupCommand {
    /// Every group the platform exposes.
    List,

    /// One group and its members.
    Info {
        /// Group name.
        name: String,
    },
}

/// Arguments for `x group`.
#[derive(Debug, clap::Args)]
pub struct GroupArgs {
    #[command(subcommand)]
    pub command: GroupCommand,
}

/// Route a `x group` invocation.
pub fn dispatch_group(
    context: &SystemContext,
    renderer: &mut Renderer,
    command: &GroupCommand,
) -> Result<i32> {
    let user = context
        .user
        .as_ref()
        .ok_or_else(|| Error::unsupported("group inspection is not available in this context"))?;

    match command {
        GroupCommand::List => {
            let groups = user.groups()?;
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&groups)?;
                return Ok(0);
            }
            let mut table = Table::new(["name", "gid", "members"]);
            for group in &groups {
                table.push(row![
                    group.name.clone(),
                    group.gid.clone().unwrap_or_else(|| "-".into()),
                    group.members.join(", "),
                ]);
            }
            renderer.table(&table)?;
        }
        GroupCommand::Info { name } => {
            let group = user.group(name)?;
            if renderer.format() == OutputFormat::Json {
                renderer.always_json(&group)?;
                return Ok(0);
            }
            let mut table = Table::new(["field", "value"]);
            table.push(row!["name", group.name.clone()]);
            if let Some(gid) = &group.gid {
                table.push(row!["gid", gid.clone()]);
            }
            table.push(row!["members", group.members.join(", ")]);
            renderer.table(&table)?;
        }
    }
    Ok(0)
}
