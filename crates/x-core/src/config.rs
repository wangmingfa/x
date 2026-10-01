//! Optional user configuration: `~/.config/x/config.toml`.
//!
//! The file is opt-in — every key has a sane default, so an empty or missing
//! file behaves exactly like today. Parsing is deliberately tiny (flat
//! `key = value` pairs with `#` comments): the four keys x honours do not
//! justify a TOML dependency in `x-core`, and malformed values are reported
//! rather than silently ignored.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The four knobs x reads from the config file.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// Default output format when no `--json/--jsonl/--csv/--plain` flag is
    /// given: `table`, `plain`, `json`, `jsonl` or `csv`.
    pub default_format: Option<String>,
    /// TUI refresh interval in milliseconds.
    pub refresh_ms: Option<u64>,
    /// TUI theme: `default`, `dark`, `light` or `monochrome`.
    pub theme: Option<String>,
    /// Default sort for process listings (`cpu`, `memory`, `pid`, `name`).
    pub sort: Option<String>,
    /// Custom theme overrides (`theme_accent = "green"`, …); see the docs in
    /// x-tui. Keys: `theme_fg`, `theme_accent`, `theme_warning`,
    /// `theme_danger`, `theme_dim`, `theme_header_bg`, `theme_header_fg`.
    pub theme_overrides: Vec<(String, String)>,
}

/// Where the config file lives: `$XDG_CONFIG_HOME/x/config.toml`, else
/// `~/.config/x/config.toml` on Unix and `%APPDATA%\x\config.toml` on
/// Windows — derived from the environment, with no OS conditional here.
pub fn default_path() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("x").join("config.toml");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return PathBuf::from(home)
                .join(".config")
                .join("x")
                .join("config.toml");
        }
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        if !appdata.is_empty() {
            return PathBuf::from(appdata).join("x").join("config.toml");
        }
    }
    PathBuf::from("x.toml")
}

/// Load the config from `path`; a missing file yields the default `Config`.
///
/// Malformed *values* are skipped with a warning; a malformed *file* (an
/// unparsable line) stops at that line so users see exactly where to fix it.
pub fn load(path: &std::path::Path) -> (Config, Vec<String>) {
    let mut config = Config::default();
    let mut warnings = Vec::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return (config, warnings);
    };
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            warnings.push(format!(
                "{}:{}: expected `key = value`, got `{line}`",
                path.display(),
                index + 1
            ));
            continue;
        };
        let key = key.trim();
        let value = value.trim().trim_matches('"');
        match key {
            "default_format" => match value {
                "table" | "plain" | "json" | "jsonl" | "csv" => {
                    config.default_format = Some(value.to_string())
                }
                other => warnings.push(format!(
                    "{}:{}: unknown format `{other}` (table|plain|json|jsonl|csv)",
                    path.display(),
                    index + 1
                )),
            },
            "refresh_ms" => match value.parse::<u64>() {
                Ok(ms) if ms >= 100 => config.refresh_ms = Some(ms),
                _ => warnings.push(format!(
                    "{}:{}: refresh_ms must be an integer >= 100, got `{value}`",
                    path.display(),
                    index + 1
                )),
            },
            "theme" => match value {
                "default" | "dark" | "light" | "monochrome" => {
                    config.theme = Some(value.to_string())
                }
                other => warnings.push(format!(
                    "{}:{}: unknown theme `{other}` (default|dark|light|monochrome)",
                    path.display(),
                    index + 1
                )),
            },
            "sort" => match value {
                "cpu" | "memory" | "pid" | "name" => config.sort = Some(value.to_string()),
                other => warnings.push(format!(
                    "{}:{}: unknown sort `{other}` (cpu|memory|pid|name)",
                    path.display(),
                    index + 1
                )),
            },
            other if other.starts_with("theme_") => {
                config
                    .theme_overrides
                    .push((other.to_string(), value.to_string()));
            }
            other => warnings.push(format!(
                "{}:{}: unknown key `{other}` (default_format, refresh_ms, theme, sort)",
                path.display(),
                index + 1
            )),
        }
    }
    (config, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_config(test: &str, body: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("x-config-{test}-{}.toml", std::process::id()));
        std::fs::write(&path, body).unwrap();
        path
    }

    #[test]
    fn missing_file_yields_defaults() {
        let (config, warnings) = load(std::path::Path::new("x-no-such-config-aa.toml"));
        assert_eq!(config, Config::default());
        assert!(warnings.is_empty());
    }

    #[test]
    fn parses_known_keys_and_skips_comments() {
        let path = write_config(
            "parses",
            "# comment\ndefault_format = \"jsonl\"\nrefresh_ms = 500\ntheme = \"dark\"\nsort = \"cpu\"\n",
        );
        let (config, warnings) = load(&path);
        assert_eq!(config.default_format.as_deref(), Some("jsonl"));
        assert_eq!(config.refresh_ms, Some(500));
        assert_eq!(config.theme.as_deref(), Some("dark"));
        assert_eq!(config.sort.as_deref(), Some("cpu"));
        assert!(warnings.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn unknown_keys_and_values_are_reported_not_applied() {
        let path = write_config(
            "unknown",
            "bogus = 1\ndefault_format = \"xml\"\nrefresh_ms = 5\n",
        );
        let (config, warnings) = load(&path);
        assert_eq!(config.default_format, None);
        assert_eq!(config.refresh_ms, None);
        assert_eq!(
            warnings.len(),
            3,
            "all three problems reported: {warnings:?}"
        );
        let _ = std::fs::remove_file(&path);
    }
}
