//! Errors shared by every platform adapter.
//!
//! The goal of this module is that a user never sees a raw OS error string.
//! Errors are classified into *categories* so that the CLI and the TUI can
//! render the same guidance on Windows, Linux and macOS.

use std::fmt;

/// Privilege level required to perform an operation.
///
/// Adapters report the *requirement* instead of a hard failure, so frontends
/// can tell the user exactly what is missing instead of only "permission
/// denied".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionRequirement {
    /// Any user may perform the operation.
    None,
    /// The operation requires extra rights granted to the current user.
    Elevated,
    /// The operation requires the super user (Linux/macOS root).
    Root,
    /// The operation requires an elevated administrator token (Windows).
    Administrator,
}

impl PermissionRequirement {
    /// Short human label, used in TUI badges.
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Elevated => "elevated",
            Self::Root => "root",
            Self::Administrator => "administrator",
        }
    }

    /// Actionable message shown to the user.
    pub const fn guidance(self) -> &'static str {
        match self {
            Self::None => "This operation is available to all users.",
            Self::Elevated => "This operation requires elevated privileges.",
            Self::Root => "This operation requires root privileges. Try running with sudo.",
            Self::Administrator => {
                "This operation requires administrator privileges. Try running as Administrator."
            }
        }
    }

    /// Concrete next step, worded for the OS the command actually ran on.
    ///
    /// Adapters report the requirement *they* know about — a shared Unix
    /// path says `Root` even when it executed on Windows through a
    /// compatibility layer — so frontends cross-check the requirement
    /// against the real host and fix the mismatch instead of telling a
    /// Windows user to reach for `sudo`.
    pub const fn platform_guidance(self, os: crate::system::OsFamily) -> &'static str {
        use crate::system::OsFamily as Os;
        match (self, os) {
            (Self::None, _) => "This operation is available to all users.",
            (Self::Administrator, Os::Windows) => {
                "Open an elevated console (Win+X, then \"Terminal (Admin)\" or \"Windows PowerShell (Admin)\") and re-run this command. If elevation is refused, check group membership with `net localgroup Administrators`."
            }
            (Self::Administrator, _) => {
                "This needs the platform's most privileged account: re-run with sudo or doas."
            }
            (Self::Root, Os::Windows) => {
                "This needs an elevated administrator console (Win+X, then \"Terminal (Admin)\"); there is no root account on Windows."
            }
            (Self::Root, Os::Linux) => {
                "Re-run with sudo (`sudo x ...`). If sudo says you are not in the sudoers file, an administrator must add you, e.g. `usermod -aG sudo $USER`."
            }
            (Self::Root, Os::MacOs) => {
                "Re-run with sudo (`sudo x ...`). If sudo succeeds yet access is still refused, check System Settings -> Privacy & Security (e.g. Full Disk Access)."
            }
            (Self::Root, Os::Bsd) => {
                "Re-run as the super user (`su -c 'x ...'`), or through doas when configured."
            }
            (Self::Root, Os::Other) => "Re-run as the super user (sudo, doas, or su).",
            (Self::Elevated, Os::Windows) => {
                "The current token lacks a privilege this operation needs: re-run from an elevated console (Win+X, then \"Terminal (Admin)\"). Controlling other users' processes requires that Administrators token."
            }
            (Self::Elevated, Os::Linux) => {
                "Re-run with sudo. For a permanent fix a single capability can be granted instead (e.g. `setcap` for raw sockets or low ports)."
            }
            (Self::Elevated, Os::MacOs) => {
                "Re-run with sudo; network or device operations may also need the app in System Settings -> Privacy & Security."
            }
            (Self::Elevated, Os::Bsd) => {
                "Re-run as root or through doas; a sysctl or capability change may be the real fix."
            }
            (Self::Elevated, Os::Other) => "This operation requires elevated privileges.",
        }
    }
}

/// Coarse error categories used for both human output and exit codes.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// The requested entity does not exist.
    NotFound,
    /// The entity exists but is not in a state that allows the operation.
    InvalidState,
    /// Missing privileges.
    PermissionDenied,
    /// A hard OS error, usually wrapped by the adapter.
    System,
    /// Input did not parse or failed validation.
    InvalidInput,
    /// A timeout expired.
    Timeout,
    /// The capability is not available on this platform/version.
    Unsupported,
    /// Anything else, and the default for values that were never set.
    #[default]
    Other,
}

impl ErrorKind {
    /// Stable process exit code. Kept small so scripts can branch on it.
    pub const fn exit_code(self) -> i32 {
        match self {
            Self::NotFound => 3,
            Self::PermissionDenied => 4,
            Self::InvalidInput => 5,
            Self::Timeout => 6,
            Self::Unsupported => 7,
            Self::InvalidState => 8,
            Self::System | Self::Other => 1,
        }
    }
}

/// The single error type crossing the `x-core` boundary.
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    /// Privilege the operation needed, if the failure was a privilege problem.
    permission: Option<PermissionRequirement>,
    message: String,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl Error {
    /// Build an error with an explicit kind and message.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            permission: None,
            message: message.into(),
            source: None,
        }
    }

    /// Entity lookup failed.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::NotFound, message)
    }

    /// User supplied something we cannot use.
    pub fn invalid_input(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::InvalidInput, message)
    }

    /// Operation failed because of current privileges.
    pub fn permission_denied(
        requirement: PermissionRequirement,
        message: impl Into<String>,
    ) -> Self {
        let mut err = Self::new(ErrorKind::PermissionDenied, message);
        err.permission = Some(requirement);
        err
    }

    /// OS level failure.
    pub fn system(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::System, message)
    }

    /// Capability missing on this platform.
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Unsupported, message)
    }

    /// Attach a permission requirement to an existing error.
    pub fn with_permission(mut self, requirement: PermissionRequirement) -> Self {
        if self.kind == ErrorKind::PermissionDenied {
            self.permission = Some(requirement);
        }
        self
    }

    /// Attach a source error for `--verbose` style output.
    pub fn with_source(mut self, source: impl std::error::Error + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self
    }

    /// Error category.
    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// Required privilege, when the error is a privilege problem.
    pub fn permission(&self) -> Option<PermissionRequirement> {
        self.permission
    }

    /// Message without the privilege hint.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Underlying OS error, if any.
    pub fn source_error(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source.as_ref().map(|e| e.as_ref() as _)
    }

    /// Process exit code for this error.
    pub fn exit_code(&self) -> i32 {
        self.kind.exit_code()
    }
}

impl From<std::io::Error> for Error {
    /// Map an I/O failure, so frontends can use `?` on any `Result` without
    /// inventing a per call site conversion.
    fn from(error: std::io::Error) -> Self {
        let kind = match error.kind() {
            std::io::ErrorKind::NotFound => ErrorKind::NotFound,
            std::io::ErrorKind::PermissionDenied => ErrorKind::PermissionDenied,
            std::io::ErrorKind::InvalidInput | std::io::ErrorKind::InvalidData => {
                ErrorKind::InvalidInput
            }
            _ => ErrorKind::System,
        };
        let mut mapped = Error::new(kind, error.to_string());
        if kind == ErrorKind::PermissionDenied {
            mapped = mapped.with_permission(PermissionRequirement::Root);
        }
        mapped.with_source(error)
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source_error()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)?;
        if let Some(req) = self.permission {
            if req != PermissionRequirement::None {
                write!(f, "\n{}", req.guidance())?;
            }
        }
        Ok(())
    }
}

/// Convenience alias used across the workspace.
pub type Result<T> = std::result::Result<T, Error>;

/// Extension trait to attach context to `Result` values.
pub trait ResultExt<T> {
    /// Prefix the error message with additional context.
    fn ctx(self, context: impl fmt::Display) -> Result<T>;
}

impl<T, E> ResultExt<T> for std::result::Result<T, E>
where
    E: std::error::Error + Send + Sync + 'static,
{
    fn ctx(self, context: impl fmt::Display) -> Result<T> {
        self.map_err(|e| Error::system(format!("{context}: {e}")).with_source(e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_error_prints_guidance() {
        let err = Error::permission_denied(
            PermissionRequirement::Administrator,
            "Cannot control service",
        );
        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
        assert_eq!(err.exit_code(), 4);
        assert!(err.to_string().contains("administrator privileges"));
    }

    #[test]
    fn platform_guidance_matches_the_real_host() {
        use crate::system::OsFamily;
        // A Unix-shaped requirement on Windows must not tell the user sudo.
        let hint = PermissionRequirement::Root.platform_guidance(OsFamily::Windows);
        assert!(!hint.contains("sudo"), "{hint}");
        assert!(hint.contains("elevated administrator console"));
        // The same host asked for an administrator reads as console steps.
        let hint = PermissionRequirement::Administrator.platform_guidance(OsFamily::Windows);
        assert!(hint.contains("net localgroup Administrators"));
        // Linux gets the sudoers recovery path.
        let hint = PermissionRequirement::Root.platform_guidance(OsFamily::Linux);
        assert!(hint.contains("sudoers"));
        // macOS adds the Privacy & Security dead end people actually hit.
        let hint = PermissionRequirement::Root.platform_guidance(OsFamily::MacOs);
        assert!(hint.contains("Privacy & Security"));
        // Every cell is a real sentence, never an empty fallback.
        for os in [
            OsFamily::Windows,
            OsFamily::Linux,
            OsFamily::MacOs,
            OsFamily::Bsd,
            OsFamily::Other,
        ] {
            for req in [
                PermissionRequirement::None,
                PermissionRequirement::Elevated,
                PermissionRequirement::Root,
                PermissionRequirement::Administrator,
            ] {
                assert!(req.platform_guidance(os).len() > 20);
            }
        }
    }

    #[test]
    fn not_found_exit_code_is_stable() {
        assert_eq!(Error::not_found("no such process").exit_code(), 3);
    }

    #[test]
    fn without_permission_kind_is_kept() {
        let err = Error::not_found("boom").with_permission(PermissionRequirement::Root);
        assert_eq!(err.permission(), None);
    }

    #[test]
    fn io_errors_keep_their_category_and_hint_at_privileges() {
        let denied: Error =
            std::io::Error::new(std::io::ErrorKind::PermissionDenied, "denied").into();
        assert_eq!(denied.kind(), ErrorKind::PermissionDenied);
        assert_eq!(denied.permission(), Some(PermissionRequirement::Root));

        let missing: Error = std::io::Error::new(std::io::ErrorKind::NotFound, "gone").into();
        assert_eq!(missing.kind(), ErrorKind::NotFound);
        assert_eq!(missing.exit_code(), 3);
    }
}
