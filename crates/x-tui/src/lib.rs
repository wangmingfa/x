//! `x-tui`: the interactive frontend.
//!
//! ```text
//! x-cli  ─┐
//!         ├─> SystemContext (dyn traits + unified models) ─> x-platform ─> OS
//! x-tui  ─┘
//! ```
//!
//! The TUI owns no system knowledge of its own: it calls the same
//! [`x_core::context::SystemContext`] the CLI does, including
//! [`x_core::port::PortManager::plan`] for killing. That is what keeps "what the
//! TUI showed me" and "what got killed" the same thing.

#![forbid(unsafe_code)]

pub mod app;
pub mod event;
pub mod palette;
pub mod theme;
pub mod ui;

use std::time::Duration;

use x_core::error::{Error, Result};
use x_core::SystemContext;

/// How often the visible snapshot is refreshed.
///
/// Watching is polling on purpose: the adapters expose snapshot reads only, so
/// the TUI, the CLI and any future HTTP API observe the exact same values.
/// `config.toml` 的 `refresh_ms`（>= 100）可以覆盖它。
pub fn refresh_interval() -> Duration {
    let (config, _) = x_core::config::load(&x_core::config::default_path());
    Duration::from_millis(config.refresh_ms.unwrap_or(REFRESH_MS))
}

const REFRESH_MS: u64 = 1500;

/// How long a key press waits before the loop redraws on its own.
pub const INPUT_TIMEOUT: Duration = Duration::from_millis(250);

/// Run the interface against `context` until the user quits.
pub fn run(context: SystemContext) -> Result<()> {
    if !crossterm::tty::IsTty::is_tty(&std::io::stdout()) {
        return Err(Error::invalid_input(
            "x-tui needs an interactive terminal; use `x` for one shot output",
        ));
    }

    // Theme (and refresh cadence) come from config.toml before any drawing.
    let (config, _) = x_core::config::load(&x_core::config::default_path());
    theme::init(config.theme.as_deref(), &config.theme_overrides);

    let mut app = app::App::new(context);
    let mut terminal = ratatui::init();
    let result = event_loop(&mut app, &mut terminal);
    ratatui::restore();
    result
}

fn event_loop(app: &mut app::App, terminal: &mut ratatui::DefaultTerminal) -> Result<()> {
    loop {
        terminal
            .draw(|frame| ui::draw(frame, app))
            .map_err(|e| Error::system(format!("cannot draw: {e}")))?;

        if app.should_quit() {
            return Ok(());
        }

        match event::next(INPUT_TIMEOUT) {
            event::Event::Key(key) => app.on_key(key),
            event::Event::Resize | event::Event::Tick => app.on_tick(),
        }
    }
}
