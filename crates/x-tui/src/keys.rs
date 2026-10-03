//! Configurable key bindings for the TUI.
//!
//! Bindings come from `config.toml` as `key_<action> = "<char>"` lines, e.g.
//! `key_quit = "Q"`. Navigation keys (arrows, Tab, Home/End, PageUp/Down,
//! Enter, Esc, Ctrl+C, Ctrl+P and the digit page jumps) stay fixed — they are
//! muscle memory from every other TUI; only the letter commands are
//! remappable. An unknown action name or a value that is not exactly one
//! character is reported as a warning and the default stays in place.

use x_core::config::Config;

/// Every remappable letter command in the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    Kill,
    Search,
    Filter,
    Refresh,
    Tree,
    Sort,
    Ports,
    Top,
    Bottom,
}

impl Action {
    /// All actions with their default key, config name and help label, in
    /// help-screen order.
    pub const ALL: &'static [(Action, char, &'static str, &'static str)] = &[
        (Action::Quit, 'q', "quit", "quit"),
        (Action::Kill, 'c', "kill", "kill selection"),
        (Action::Search, '/', "search", "search"),
        (Action::Filter, 'f', "filter", "filter"),
        (Action::Refresh, 'r', "refresh", "refresh"),
        (Action::Tree, 't', "tree", "tree mode"),
        (Action::Sort, 's', "sort", "cycle sort"),
        (Action::Ports, 'p', "ports", "ports of selection"),
        (Action::Top, 'g', "top", "jump to top"),
        (Action::Bottom, 'G', "bottom", "jump to bottom"),
    ];

    /// The config-file name (`key_<name> = …`).
    fn name(self) -> &'static str {
        let (_, _, name, _) = Self::ALL
            .iter()
            .find(|(a, _, _, _)| *a == self)
            .expect("known");
        name
    }

    /// Parse an action from its config name, e.g. `"quit"` -> `Action::Quit`.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .find(|(_, _, n, _)| *n == name)
            .map(|(a, _, _, _)| *a)
    }
}

/// The effective bindings: defaults overridden by the config file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keys {
    map: Vec<(Action, char)>,
}

impl Default for Keys {
    fn default() -> Self {
        Self {
            map: Action::ALL.iter().map(|(a, ch, _, _)| (*a, *ch)).collect(),
        }
    }
}

impl Keys {
    /// Apply `key_<action> = "X"` pairs from the config; unknown actions and
    /// malformed values are reported as warnings and skipped.
    pub fn from_config(config: &Config) -> (Self, Vec<String>) {
        let mut keys = Self::default();
        let mut warnings = Vec::new();
        for (key, value) in &config.keys {
            let Some(action) = key.strip_prefix("key_").and_then(Action::from_name) else {
                warnings.push(format!("unknown action `{key}` (try quit, kill, search, filter, refresh, tree, sort, ports, top, bottom)"));
                continue;
            };
            let mut chars = value.chars();
            match (chars.next(), chars.next()) {
                (Some(ch), None) => {
                    if let Some(slot) = keys.map.iter_mut().find(|(a, _)| *a == action) {
                        slot.1 = ch;
                    }
                }
                _ => warnings.push(format!(
                    "key_{} must be exactly one character, got `{value}`",
                    action.name()
                )),
            }
        }
        (keys, warnings)
    }

    /// The key bound to `action`.
    pub fn key(&self, action: Action) -> char {
        self.map
            .iter()
            .find(|(a, _)| *a == action)
            .map(|(_, ch)| *ch)
            .expect("bound")
    }

    /// Whether `key` triggers `action`.
    pub fn matches(&self, action: Action, key: char) -> bool {
        self.key(action) == key
    }

    /// The key and label for each action, for the help footer.
    pub fn legend(&self) -> Vec<(char, &'static str)> {
        Action::ALL
            .iter()
            .map(|(a, _, _, label)| (self.key(*a), *label))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with(pairs: &[(&str, &str)]) -> Config {
        Config {
            keys: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn defaults_match_today() {
        let keys = Keys::default();
        assert_eq!(keys.key(Action::Quit), 'q');
        assert_eq!(keys.key(Action::Kill), 'c');
        assert_eq!(keys.key(Action::Search), '/');
        assert!(keys.matches(Action::Refresh, 'r'));
    }

    #[test]
    fn overrides_apply_and_match() {
        let (keys, warnings) =
            Keys::from_config(&config_with(&[("key_quit", "Q"), ("key_kill", "x")]));
        assert!(warnings.is_empty());
        assert_eq!(keys.key(Action::Quit), 'Q');
        assert_eq!(keys.key(Action::Kill), 'x');
        assert!(keys.matches(Action::Quit, 'Q'));
        assert!(!keys.matches(Action::Quit, 'q'));
        // Untouched actions keep their defaults.
        assert_eq!(keys.key(Action::Search), '/');
    }

    #[test]
    fn unknown_action_and_bad_value_are_warnings_not_errors() {
        let (keys, warnings) =
            Keys::from_config(&config_with(&[("key_fly", "w"), ("key_quit", "quit")]));
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert_eq!(keys.key(Action::Quit), 'q', "default stays in place");
    }
}
