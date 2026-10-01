//! Where the OS keeps its hosts file.
//!
//! The file format is identical everywhere and `x-core` parses it; choosing
//! the location is OS knowledge, so it lives here rather than in the model.

use std::path::PathBuf;

/// The platform hosts file location.
pub fn default_path() -> PathBuf {
    #[cfg(windows)]
    {
        std::env::var("SystemRoot")
            .map(|root| PathBuf::from(root).join(r"System32\drivers\etc\hosts"))
            .unwrap_or_else(|_| PathBuf::from(r"C:\Windows\System32\drivers\etc\hosts"))
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/etc/hosts")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_path_points_at_the_real_hosts_file() {
        let path = default_path();
        #[cfg(windows)]
        assert_eq!(
            path.file_name().map(|n| n.to_string_lossy().into_owned()),
            Some("hosts".into())
        );
        #[cfg(not(windows))]
        assert_eq!(path, PathBuf::from("/etc/hosts"));
    }
}
