//! Terminal input, reduced to what this interface needs.
//!
//! Reading keys and redrawing happen on the same thread, so input is polled with
//! a timeout instead of an async runtime: no dependency, no executor, and the
//! loop stays a plain `loop`.

use std::time::Duration;

use crossterm::event::{self, KeyEvent, KeyEventKind};

/// What happened while the interface was waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// A key was pressed.
    Key(KeyEvent),
    /// The terminal changed size.
    Resize,
    /// Nothing happened before the timeout, so it is time to refresh.
    Tick,
}

/// Wait up to `timeout` for one event.
pub fn next(timeout: Duration) -> Event {
    match event::poll(timeout) {
        Ok(true) => match event::read() {
            // Key releases and auto repeats are noise for a snapshot interface.
            Ok(event::Event::Key(key)) if key.kind == KeyEventKind::Press => Event::Key(key),
            Ok(_) => Event::Tick,
            Err(_) => Event::Tick,
        },
        // A failed poll means the terminal is gone; treat it as a tick and let
        // the draw fail with a real message instead of spinning silently.
        Ok(false) | Err(_) => Event::Tick,
    }
}
