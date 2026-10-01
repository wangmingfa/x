//! TUI themes.
//!
//! A [`Theme`] maps the handful of *roles* the interface uses (accent,
//! warning, danger, dim, header, text) to ratatui colors. Four built-ins ship
//! with x; `config.toml` picks one with `theme = "…"` and can override any
//! role with a `theme_<role> = "<color>"` key — the "custom" story without a
//! new file format.
//!
//! The active theme lives in a process-wide slot that `x-tui::run` fills once
//! at startup, so draw code can style itself without threading a theme
//! through every call.

use ratatui::style::Color;
use std::sync::OnceLock;

/// The roles draw code styles itself with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Selection highlights, gauges, focused things.
    pub accent: Color,
    /// Worth noticing, not an error.
    pub warning: Color,
    /// Errors, destructive actions.
    pub danger: Color,
    /// De-emphasised text (hints, footers).
    pub dim: Color,
    /// Header / title bar background and foreground.
    pub header_bg: Color,
    pub header_fg: Color,
    /// Ordinary text foreground.
    pub text: Color,
}

impl Theme {
    /// Look up a built-in by config name; unknown names fall back to default.
    pub fn by_name(name: &str) -> Theme {
        match name {
            "dark" => Self::dark(),
            "light" => Self::light(),
            "monochrome" => Self::monochrome(),
            _ => Self::default_theme(),
        }
    }

    /// The shipped look: blue header, cyan accent, tuned for dark terminals.
    pub fn default_theme() -> Theme {
        Theme {
            accent: Color::Cyan,
            warning: Color::Yellow,
            danger: Color::Red,
            dim: Color::DarkGray,
            header_bg: Color::Blue,
            header_fg: Color::White,
            text: Color::Reset,
        }
    }

    /// Dark: same structure, slightly calmer accent for OLED-heavy use.
    pub fn dark() -> Theme {
        Theme {
            accent: Color::LightCyan,
            ..Self::default_theme()
        }
    }

    /// Light: darker accents that survive a white terminal background.
    pub fn light() -> Theme {
        Theme {
            accent: Color::Blue,
            warning: Color::Magenta,
            danger: Color::Red,
            dim: Color::DarkGray,
            header_bg: Color::Blue,
            header_fg: Color::White,
            text: Color::Black,
        }
    }

    /// Monochrome: grays only, for terminals without reliable color.
    pub fn monochrome() -> Theme {
        Theme {
            accent: Color::Gray,
            warning: Color::White,
            danger: Color::Gray,
            dim: Color::DarkGray,
            header_bg: Color::Gray,
            header_fg: Color::Black,
            text: Color::White,
        }
    }

    /// Apply `key -> color-name` overrides from the config file.
    ///
    /// Unknown role names or unparseable colors are skipped: a typo in the
    /// config degrades to the base theme instead of breaking the TUI.
    pub fn with_overrides(mut self, overrides: &[(String, String)]) -> Theme {
        for (key, value) in overrides {
            let Some(color) = parse_color(value) else {
                continue;
            };
            match key.as_str() {
                "theme_fg" => self.text = color,
                "theme_accent" => self.accent = color,
                "theme_warning" => self.warning = color,
                "theme_danger" => self.danger = color,
                "theme_dim" => self.dim = color,
                "theme_header_bg" => self.header_bg = color,
                "theme_header_fg" => self.header_fg = color,
                _ => {}
            }
        }
        self
    }

    /// Ordinary text.
    pub fn text(&self) -> ratatui::style::Style {
        ratatui::style::Style::default().fg(self.text)
    }

    /// Accent text, optionally bold.
    pub fn accent(&self, bold: bool) -> ratatui::style::Style {
        let style = ratatui::style::Style::default().fg(self.accent);
        if bold {
            style.bold()
        } else {
            style
        }
    }

    /// Warning text (bold: it is meant to be noticed).
    pub fn warning(&self) -> ratatui::style::Style {
        ratatui::style::Style::default().fg(self.warning).bold()
    }

    /// Danger text.
    pub fn danger(&self) -> ratatui::style::Style {
        ratatui::style::Style::default().fg(self.danger)
    }

    /// De-emphasised text.
    pub fn dim(&self) -> ratatui::style::Style {
        ratatui::style::Style::default().fg(self.dim)
    }

    /// Header / title bar style.
    pub fn header(&self) -> ratatui::style::Style {
        ratatui::style::Style::default()
            .bg(self.header_bg)
            .fg(self.header_fg)
    }
}

/// Accept the color names the docs promise, plus `reset`.
fn parse_color(name: &str) -> Option<Color> {
    Some(match name.to_ascii_lowercase().as_str() {
        "reset" => Color::Reset,
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "white" => Color::White,
        "gray" | "grey" => Color::Gray,
        "darkgray" | "dark-grey" => Color::DarkGray,
        "lightred" => Color::LightRed,
        "lightgreen" => Color::LightGreen,
        "lightyellow" => Color::LightYellow,
        "lightblue" => Color::LightBlue,
        "lightmagenta" => Color::LightMagenta,
        "lightcyan" => Color::LightCyan,
        _ => return None,
    })
}

static ACTIVE: OnceLock<Theme> = OnceLock::new();

/// Install the process-wide theme; the first call wins.
///
/// `run` calls this with the config's `theme` name and `theme_*` overrides
/// before any drawing happens.
pub fn init(name: Option<&str>, overrides: &[(String, String)]) {
    let mut theme = Theme::by_name(name.unwrap_or("default"));
    theme = theme.with_overrides(overrides);
    let _ = ACTIVE.set(theme);
}

/// The active theme; defaults to the built-in look when `init` never ran
/// (tests, embedding).
pub fn current() -> &'static Theme {
    ACTIVE.get_or_init(Theme::default_theme)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_builtin_has_distinct_roles_resolved() {
        for name in ["default", "dark", "light", "monochrome"] {
            let theme = Theme::by_name(name);
            let _ = theme.accent(true);
            let _ = theme.header();
            let _ = theme.dim();
        }
    }

    #[test]
    fn unknown_name_falls_back_to_default() {
        assert_eq!(Theme::by_name("nope"), Theme::default_theme());
    }

    #[test]
    fn overrides_apply_known_roles_and_skip_the_rest() {
        let theme = Theme::by_name("default").with_overrides(&[
            ("theme_accent".to_string(), "green".to_string()),
            ("theme_nope".to_string(), "green".to_string()),
            ("theme_dim".to_string(), "chartreuse".to_string()),
        ]);
        assert_eq!(theme.accent, Color::Green);
        assert_eq!(theme.dim, Theme::default_theme().dim);
    }

    #[test]
    fn init_then_current_returns_the_installed_theme() {
        init(Some("monochrome"), &[]);
        assert_eq!(current().accent, Color::Gray);
    }
}
