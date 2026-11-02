//! Finder: a search-first view of every session (`mockups/rs/finder`). Every printable key is
//! query text; nucleo matches it fuzzily against each session's title, project name, project
//! path and folder, and each field lights up where it matched. Results come grouped by state,
//! needs-you first; ended sessions join in whenever a query is typed. Beside the list (below it
//! under 110 columns) a large preview: your ask and Claude's reply as chat bubbles, what the
//! session is waiting on, its context and output, and the details.
//!
//! Keys: letters type; ↑/↓, Ctrl+P/Ctrl+N, PgUp/PgDn, Home/End move; ←/→ move in the query;
//! Ctrl+U/Ctrl+W clear it / delete a word; Esc clears it, then goes back. Enter, Ctrl+S,
//! Ctrl+X and Ctrl+E fall through to the app's global table; Ctrl+K is the action menu
//! ([`menu`]).

mod draw;
pub mod matching;
pub mod menu;

pub use menu::{ActionMenu, MenuRow, menu_rows};

use super::step;
use crate::actions::Cursor;
use crate::facts::Session;
use crate::tui::app::{Action, Ctx, View};
use crate::tui::widgets::{Edit, LineEdit};
use matching::{Hit, Search};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

#[derive(Default)]
pub struct Finder {
    query: LineEdit,
    search: Search,
    /// The results list's first line on screen, kept between frames so the list only scrolls
    /// when the selection would leave it.
    scroll: usize,
    /// Results that fit on screen at the last draw: what PgUp/PgDn move by.
    page: usize,
}

impl Finder {
    /// The query as typed.
    pub fn query(&self) -> &str {
        self.query.text()
    }

    fn hits(&mut self, cx: &Ctx) -> Vec<Hit> {
        self.search.hits(self.query.text(), &cx.snap.sessions, cx.prefs.show_ended, cx.home, cx.scratch)
    }

    fn rows<'a>(hits: &[Hit], cx: &Ctx<'a>) -> Vec<&'a Session> {
        hits.iter().map(|h| &cx.snap.sessions[h.index]).collect()
    }
}

impl View for Finder {
    fn draw(&mut self, frame: &mut Frame, area: Rect, cx: &Ctx, cursor: &mut Option<Cursor>) {
        let hits = self.hits(cx);
        let rows = Self::rows(&hits, cx);
        let sel = super::resolve(&rows, cursor);
        let out = draw::draw(
            frame.buffer_mut(),
            area,
            cx,
            &draw::State { query: &self.query, hits: &hits, sel, scroll: self.scroll },
        );
        self.scroll = out.scroll;
        self.page = out.page.max(1);
    }

    fn handle_key(&mut self, key: KeyEvent, cx: &Ctx, cursor: &mut Option<Cursor>) -> Option<Action> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let hits = self.hits(cx);
        let rows = Self::rows(&hits, cx);
        let page = self.page.max(1) as isize;
        let delta = match key.code {
            KeyCode::Up => Some(-1),
            KeyCode::Down => Some(1),
            KeyCode::Char('p') if ctrl => Some(-1),
            KeyCode::Char('n') if ctrl => Some(1),
            KeyCode::PageUp => Some(-page),
            KeyCode::PageDown => Some(page),
            KeyCode::Home => Some(isize::MIN / 2),
            KeyCode::End => Some(isize::MAX / 2),
            _ => None,
        };
        if let Some(d) = delta {
            step(&rows, cursor, d);
            return Some(Action::Handled);
        }
        match key.code {
            KeyCode::Esc if !self.query.is_empty() => {
                self.query.clear();
                return Some(Action::Handled);
            }
            // The app opens `ActionMenu` as an overlay.
            KeyCode::Char('k') if ctrl => {
                let _ = super::resolve(&rows, cursor);
                return Some(Action::Menu);
            }
            // These act on the selected session: only ever one the list shows.
            KeyCode::Enter => return on_selected(&rows, cursor),
            KeyCode::Char('s' | 'x') if ctrl => return on_selected(&rows, cursor),
            _ => {}
        }
        match self.query.handle(&key) {
            Edit::Ignored => None,
            Edit::Moved => Some(Action::Handled),
            Edit::Changed => {
                // As in fzf, a new query puts you on its best result.
                let hits = self.hits(cx);
                if let Some(h) = hits.first() {
                    *cursor = Some(Cursor::Session(cx.snap.sessions[h.index].session_id.clone()));
                }
                self.scroll = 0;
                Some(Action::Handled)
            }
        }
    }

    fn captures_text(&self) -> bool {
        true
    }

    fn shown(&mut self, search: bool) {
        if search {
            // The cursor goes to the end of whatever was typed last time.
            let q = self.query.text().to_string();
            self.query.set(&q);
        }
    }
}

/// For Enter, Ctrl+S and Ctrl+X: settle the cursor on a listed session and let the app's
/// global table act on it; with nothing listed, there's nothing to act on.
fn on_selected(rows: &[&Session], cursor: &mut Option<Cursor>) -> Option<Action> {
    match super::resolve(rows, cursor) {
        Some(_) => None,
        None => Some(Action::Toast("Nothing matches the search.".into())),
    }
}
