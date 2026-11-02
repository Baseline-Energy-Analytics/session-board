//! Taking the terminal and giving it back: raw mode and the alternate screen, and nothing else.
//! No mouse capture, bracketed paste or keyboard-enhancement flags, so there is nothing more to
//! undo when `claude attach` takes over the terminal.
//!
//! These are ratatui 0.30's `init()`/`restore()` steps, split so the suspend path re-enters with
//! the same function as start-up: `ratatui::init()` would also install another panic hook on
//! every return from a session.

use ratatui::DefaultTerminal;
use ratatui::crossterm::cursor::Show;
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::prelude::CrosstermBackend;
use std::io::{self, stdout};
use std::sync::Once;

/// Take the terminal: a panic hook that gives it back first (once per process), raw mode, the
/// alternate screen.
pub fn init() -> io::Result<DefaultTerminal> {
    static HOOK: Once = Once::new();
    HOOK.call_once(|| {
        let next = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // The collect thread's panics are caught and shown as a notice; printing them
            // would scribble over the board.
            if matches!(std::thread::current().name(), Some(super::worker::THREAD | super::CLI_THREAD)) {
                return;
            }
            restore();
            next(info);
        }));
    });
    enter()?;
    ratatui::Terminal::new(CrosstermBackend::new(stdout()))
}

fn enter() -> io::Result<()> {
    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen)
}

/// Give the terminal back as it was. Errors are ignored: there's nothing better to do with them
/// on the way out.
pub fn restore() {
    let _ = disable_raw_mode();
    let _ = execute!(stdout(), LeaveAlternateScreen, Show);
}

/// Hand the terminal to `run` (a `claude attach` with inherited stdio) and take it back
/// afterwards. `terminal.clear()` makes the next draw repaint everything, since the screen no
/// longer holds the frame ratatui last drew; it also picks up a resize made while away.
pub fn suspend_for<T>(terminal: &mut DefaultTerminal, run: impl FnOnce() -> T) -> io::Result<T> {
    restore();
    let out = run();
    enter()?;
    terminal.clear()?;
    Ok(out)
}
