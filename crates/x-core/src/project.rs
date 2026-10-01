//! Project detection.
//!
//! A project is whatever the directory says it is: marker files and lockfiles,
//! the same signal every build tool itself uses. No manifest parsing beyond a
//! few string fields — the goal is "what is this and how do I start it", not
//! reimplementing package managers.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Package manager / build tool a project uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageManager {
    Npm,
    Pnpm,
    Yarn,
    Bun,
    Cargo,
    Pip,
    Poetry,
    Uv,
    GoModules,
    Maven,
    Gradle,
    Composer,
    Unknown,
}

impl PackageManager {
    /// Lowercase identifier.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Npm => "npm",
            Self::Pnpm => "pnpm",
            Self::Yarn => "yarn",
            Self::Bun => "bun",
            Self::Cargo => "cargo",
            Self::Pip => "pip",
            Self::Poetry => "poetry",
            Self::Uv => "uv",
            Self::GoModules => "go",
            Self::Maven => "maven",
            Self::Gradle => "gradle",
            Self::Composer => "composer",
            Self::Unknown => "unknown",
        }
    }

    /// Command that would build / run this project, when there is one.
    pub const fn command(self) -> Option<&'static str> {
        match self {
            Self::Npm => Some("npm"),
            Self::Pnpm => Some("pnpm"),
            Self::Yarn => Some("yarn"),
            Self::Bun => Some("bun"),
            Self::Cargo => Some("cargo"),
            Self::Pip | Self::Poetry | Self::Uv => Some("python"),
            Self::GoModules => Some("go"),
            Self::Maven => Some("mvn"),
            Self::Gradle => Some("gradle"),
            Self::Composer => Some("composer"),
            Self::Unknown => None,
        }
    }
}

/// One detected project.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectInfo {
    /// Project root the detection started from.
    pub root: PathBuf,
    /// Language / ecosystem markers found (`rust`, `node`, `python`, ...).
    pub kinds: Vec<String>,
    /// The most specific package manager found.
    pub package_manager: Option<PackageManager>,
    /// Project name from the best available manifest.
    pub name: Option<String>,
    /// Detected version string from the manifest.
    pub version: Option<String>,
    /// Current git branch, when the project is inside one.
    pub branch: Option<String>,
    /// Entry points that look like the way to run it (`src/main.rs`, `package.json`, ...).
    pub entry_points: Vec<String>,
}

/// Inspect `dir` (walking up to 6 parents) and report what kind of project
/// lives there.
pub fn detect(dir: &Path) -> ProjectInfo {
    let root = find_project_root(dir).unwrap_or_else(|| dir.to_path_buf());
    let mut info = ProjectInfo {
        root,
        ..ProjectInfo::default()
    };

    let markers: [(&str, &str); 12] = [
        ("Cargo.toml", "rust"),
        ("package.json", "node"),
        ("deno.json", "deno"),
        ("go.mod", "go"),
        ("requirements.txt", "python"),
        ("pyproject.toml", "python"),
        ("pom.xml", "java"),
        ("build.gradle", "java"),
        ("composer.json", "php"),
        ("Gemfile", "ruby"),
        ("mix.exs", "elixir"),
        ("docker-compose.yml", "docker"),
    ];
    for (file, kind) in markers {
        if info.root.join(file).is_file() {
            info.kinds.push(kind.to_string());
        }
    }
    info.kinds.dedup();

    info.package_manager = package_manager(&info.root);
    let manifest = read_manifest(&info.root);
    if info.name.is_none() {
        info.name = manifest.as_ref().and_then(|m| m.0.clone());
    }
    if info.version.is_none() {
        info.version = manifest.as_ref().and_then(|m| m.1.clone());
    }

    info.branch = crate::gitcmd::status(&info.root)
        .ok()
        .and_then(|s| s.branch);

    for entry in [
        "src/main.rs",
        "src/lib.rs",
        "package.json",
        "index.js",
        "src/index.ts",
        "main.py",
        "app.py",
        "main.go",
        "Makefile",
    ] {
        if info.root.join(entry).is_file() {
            info.entry_points.push(entry.to_string());
        }
    }

    info
}

/// Nearest ancestor (including `dir`) that contains a known manifest, for
/// "which project am I actually in".
pub fn find_project_root(dir: &Path) -> Option<PathBuf> {
    let markers = [
        "Cargo.toml",
        "package.json",
        "go.mod",
        "pyproject.toml",
        "pom.xml",
        ".git",
    ];
    let mut current = Some(dir);
    let mut depth = 0;
    while let Some(path) = current {
        if depth > 6 {
            return None;
        }
        if markers.iter().any(|m| path.join(m).exists()) {
            return Some(path.to_path_buf());
        }
        current = path.parent();
        depth += 1;
    }
    None
}

fn package_manager(root: &Path) -> Option<PackageManager> {
    let lockfiles: [(&str, PackageManager); 10] = [
        ("pnpm-lock.yaml", PackageManager::Pnpm),
        ("yarn.lock", PackageManager::Yarn),
        ("bun.lockb", PackageManager::Bun),
        ("Cargo.lock", PackageManager::Cargo),
        ("poetry.lock", PackageManager::Poetry),
        ("uv.lock", PackageManager::Uv),
        ("go.sum", PackageManager::GoModules),
        ("package-lock.json", PackageManager::Npm),
        ("composer.lock", PackageManager::Composer),
        ("requirements.txt", PackageManager::Pip),
    ];
    lockfiles
        .into_iter()
        .find(|(file, _)| root.join(file).is_file())
        .map(|(_, manager)| manager)
}

/// `(name, version)` from Cargo.toml / package.json / pyproject.toml / go.mod.
///
/// Deliberately string-slicing, not TOML/JSON parsing: a one line read of two
/// fields cannot justify two more dependencies in x-core.
fn read_manifest(root: &Path) -> Option<(Option<String>, Option<String>)> {
    let sources = ["Cargo.toml", "package.json", "pyproject.toml", "go.mod"];
    for file in sources {
        let Ok(text) = std::fs::read_to_string(root.join(file)) else {
            continue;
        };
        let mut name = None;
        let mut version = None;
        for line in text.lines() {
            let line = line.trim().trim_start_matches('{').trim();
            if let Some(rest) = line.strip_prefix("\"name\": \"") {
                name = Some(rest.split('"').next().unwrap_or_default().to_string());
            } else if let Some(rest) = line.strip_prefix("name = \"") {
                name = Some(rest.trim_end_matches('"').to_string());
            } else if let Some(rest) = line.strip_prefix("\"version\": \"") {
                version = Some(rest.split('"').next().unwrap_or_default().to_string());
            } else if let Some(rest) = line.strip_prefix("version = \"") {
                version = Some(rest.trim_end_matches('"').to_string());
            } else if let Some(rest) = line.strip_prefix("module ") {
                name = Some(rest.trim().to_string());
            }
            if name.is_some() && version.is_some() {
                break;
            }
        }
        if name.is_some() || version.is_some() {
            return Some((name, version));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("x-core-project-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn detects_rust_project_with_cargo_lock() {
        let dir = temp_dir("rust");
        std::fs::write(
            dir.join("Cargo.toml"),
            "[package]\nname = \"demo\"\nversion = \"1.2.3\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("Cargo.lock"), "").unwrap();
        std::fs::create_dir(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}").unwrap();

        let info = detect(&dir);
        assert!(info.kinds.contains(&"rust".to_string()));
        assert_eq!(info.package_manager, Some(PackageManager::Cargo));
        assert_eq!(info.name.as_deref(), Some("demo"));
        assert_eq!(info.version.as_deref(), Some("1.2.3"));
        assert!(info.entry_points.contains(&"src/main.rs".to_string()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_directory_is_detected_as_nothing_in_particular() {
        let dir = temp_dir("empty");
        let info = detect(&dir);
        assert!(info.kinds.is_empty());
        assert_eq!(info.package_manager, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn node_project_prefers_pnpm_when_its_lockfile_is_present() {
        let dir = temp_dir("node");
        std::fs::write(
            dir.join("package.json"),
            "{\"name\": \"web\", \"version\": \"0.1.0\"}",
        )
        .unwrap();
        std::fs::write(dir.join("pnpm-lock.yaml"), "").unwrap();

        let info = detect(&dir);
        assert_eq!(info.package_manager, Some(PackageManager::Pnpm));
        assert_eq!(info.name.as_deref(), Some("web"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
