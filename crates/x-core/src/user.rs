//! Users and groups.
//!
//! Windows (SAM / local accounts), Linux (`/etc/passwd`, `getent`) and macOS
//! (Directory Service) model identity differently; x reports what the platform
//! exposes and leaves the rest `None` instead of inventing values.

use crate::error::Result;
use serde::{Deserialize, Serialize};

/// One user account.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UserInfo {
    /// Login name.
    pub name: String,
    /// Numeric user id (Unix); `None` on Windows.
    pub uid: Option<String>,
    /// Numeric primary group id (Unix); `None` on Windows.
    pub gid: Option<String>,
    /// Group names the user belongs to, when cheaply available.
    pub groups: Vec<String>,
    /// Full / display name.
    pub full_name: Option<String>,
    /// Home directory.
    pub home: Option<String>,
    /// Login shell.
    pub shell: Option<String>,
}

/// One group and its members.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GroupInfo {
    /// Group name.
    pub name: String,
    /// Numeric group id (Unix); `None` on Windows.
    pub gid: Option<String>,
    /// Member names.
    pub members: Vec<String>,
}

/// User and group inspection.
pub trait UserManager: Send + Sync {
    /// The account this process runs as.
    fn current(&self) -> Result<UserInfo>;

    /// Every account the platform is willing to enumerate. Servers often
    /// expose many; callers should filter or limit.
    fn list(&self) -> Result<Vec<UserInfo>>;

    /// One account by name. Default: scan [`Self::list`].
    fn info(&self, name: &str) -> Result<UserInfo> {
        self.list()?
            .into_iter()
            .find(|user| user.name == name)
            .ok_or_else(|| crate::Error::not_found(format!("no such user: {name}")))
    }

    /// Every group the platform exposes.
    fn groups(&self) -> Result<Vec<GroupInfo>>;

    /// One group by name, members included. Default: scan [`Self::groups`].
    fn group(&self, name: &str) -> Result<GroupInfo> {
        self.groups()?
            .into_iter()
            .find(|group| group.name == name)
            .ok_or_else(|| crate::Error::not_found(format!("no such group: {name}")))
    }
}
