//! The command palette: every action the interface can run, searchable by
//! name.
//!
//! The table is static and pure data so it can be filtered and tested without
//! a terminal; the application maps [`CommandId`] to its handlers, which keeps
//! palette execution on exactly the same code path as the key bindings.

use crate::app::View;

/// One palette command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Command {
    /// What the app should run.
    pub id: CommandId,
    /// What the user sees and searches.
    pub label: &'static str,
    /// The key binding the command mirrors.
    pub hint: &'static str,
}

/// What a palette command does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandId {
    /// Switch to a page.
    Goto(View),
    /// Re-read the current snapshot.
    Refresh,
    /// Ask for a filter string.
    Filter,
    /// Open the global search.
    Search,
    /// Ask to kill the selection.
    Kill,
    /// Toggle the process tree.
    ToggleTree,
    /// Cycle the process sort key.
    CycleSort,
    /// Jump to the ports held by the selected process.
    PortsOfSelection,
    /// Leave the interface.
    Quit,
}

/// Every command, in display order.
pub fn commands() -> Vec<Command> {
    let mut out: Vec<Command> = View::ALL
        .iter()
        .map(|view| Command {
            id: CommandId::Goto(*view),
            label: goto_label(*view),
            hint: goto_hint(*view),
        })
        .collect();
    out.extend([
        Command {
            id: CommandId::Search,
            label: "global search",
            hint: "/",
        },
        Command {
            id: CommandId::Filter,
            label: "filter this page",
            hint: "f",
        },
        Command {
            id: CommandId::Refresh,
            label: "refresh",
            hint: "r",
        },
        Command {
            id: CommandId::Kill,
            label: "kill selection",
            hint: "k",
        },
        Command {
            id: CommandId::ToggleTree,
            label: "processes: toggle tree",
            hint: "t",
        },
        Command {
            id: CommandId::CycleSort,
            label: "processes: cycle sort key",
            hint: "s",
        },
        Command {
            id: CommandId::PortsOfSelection,
            label: "processes: show held ports",
            hint: "p",
        },
        Command {
            id: CommandId::Quit,
            label: "quit",
            hint: "q",
        },
    ]);
    out
}

/// Commands whose label contains `needle` (case-insensitive); an empty needle
/// matches everything.
pub fn matching(needle: &str) -> Vec<Command> {
    let needle = needle.trim().to_ascii_lowercase();
    commands()
        .into_iter()
        .filter(|command| needle.is_empty() || command.label.contains(&needle))
        .collect()
}

fn goto_label(view: View) -> &'static str {
    match view {
        View::Dashboard => "go to dashboard",
        View::Ports => "go to ports",
        View::Processes => "go to processes",
        View::Network => "go to network",
        View::Services => "go to services",
        View::System => "go to system",
        View::Disks => "go to disks",
        View::Remote => "go to remote hosts",
    }
}

/// The digit key that jumps straight to the same page.
fn goto_hint(view: View) -> &'static str {
    match view {
        View::Dashboard => "1",
        View::Ports => "2",
        View::Processes => "3",
        View::Network => "4",
        View::Services => "5",
        View::System => "6",
        View::Disks => "7",
        View::Remote => "8",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_view_has_a_goto_command_with_a_unique_digit_hint() {
        let commands = commands();
        for view in View::ALL {
            let command = commands
                .iter()
                .find(|command| command.id == CommandId::Goto(view))
                .unwrap_or_else(|| panic!("no command for {view:?}"));
            assert!(!command.label.is_empty());
            assert!(!command.hint.is_empty());
        }
        let mut hints: Vec<&str> = commands
            .iter()
            .filter(|command| matches!(command.id, CommandId::Goto(_)))
            .map(|command| command.hint)
            .collect();
        hints.sort_unstable();
        assert_eq!(hints, ["1", "2", "3", "4", "5", "6", "7", "8"]);
    }

    #[test]
    fn matching_is_case_insensitive_and_empty_matches_everything() {
        assert_eq!(matching("").len(), commands().len());
        let hits = matching("TREE");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, CommandId::ToggleTree);
        assert!(matching("nothing here").is_empty());
    }
}
