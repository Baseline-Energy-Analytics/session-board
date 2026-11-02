//! Finder's Ctrl+K action menu: what can be done to the selected session, each row showing the
//! exact `claude` command it runs or, greyed, why `actions.rs` refuses it.
//!
//! The menu is an overlay, which belongs to the app: `Overlay::Menu` in `overlays.rs`, opened
//! by `Action::Menu` (Finder's Ctrl+K). This file holds what the variant runs:
//! [`ActionMenu::new`] builds it from the cursor, [`ActionMenu::handle_key`] answers in the
//! overlays' [`Reply`], and [`ActionMenu::draw`] draws it. Commands show the claude command as
//! configured (`ssh box claude …`).

use crate::actions::{self, Cursor, Enter};
use crate::facts::Session;
use crate::tui::app::{Action, Ctx};
use crate::tui::overlays::Reply;
use crate::tui::theme::{Theme, fg};
use crate::tui::widgets::{fill, popup, put, put_right, put_str, trunc, wrap};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// One row of the menu.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuRow {
    /// The key that picks it inside the menu (and its shortcut outside, where there is one).
    pub key: &'static str,
    pub label: String,
    /// The command it runs, or why it can't be done.
    pub detail: String,
    /// `None` when refused: the detail says why.
    pub action: Option<Action>,
}

impl MenuRow {
    fn new(key: &'static str, label: &str, detail: impl Into<String>, action: Option<Action>) -> MenuRow {
        MenuRow { key, label: label.into(), detail: detail.into(), action }
    }

    pub fn allowed(&self) -> bool {
        self.action.is_some()
    }
}

/// The rows for session `s` (`None`: nothing selected). The rules are `actions::plan_*`'s; the
/// commands are the ones the app runs.
pub fn menu_rows(s: Option<&Session>, cx: &Ctx) -> Vec<MenuRow> {
    let mut rows = Vec::new();
    let claude = cx.claude_cmd();
    match s {
        Some(s) => {
            let id = s.session_id.clone();
            rows.push(match actions::plan_enter(s) {
                Ok(Enter::Attach { job }) => {
                    MenuRow::new("↵", "Jump in", format!("{claude} attach {job}"), Some(Action::Jump(id.clone())))
                }
                Ok(Enter::ResumeThenAttach { session_id, .. }) => MenuRow::new(
                    "↵",
                    "Resume and jump in",
                    format!("{claude} --bg --resume {session_id}, then {claude} attach"),
                    Some(Action::Jump(id.clone())),
                ),
                Err(why) => MenuRow::new("↵", "Jump in", why.0, None),
            });
            rows.push(match actions::plan_stop(s) {
                Ok(job) => MenuRow::new("s", "Stop", format!("{claude} stop {job}"), Some(Action::Stop(id.clone()))),
                Err(why) => MenuRow::new("s", "Stop", why.0, None),
            });
            rows.push(match actions::plan_delete(s, cx.days) {
                Ok(job) => MenuRow::new("x", "Delete…", format!("{claude} rm {job}"), Some(Action::Delete(id))),
                Err(why) => MenuRow::new("x", "Delete…", why.0, None),
            });
        }
        None => {
            for (k, label) in [("↵", "Jump in"), ("s", "Stop"), ("x", "Delete…")] {
                rows.push(MenuRow::new(k, label, "No session is selected.", None));
            }
        }
    }
    rows.push(MenuRow::new("n", "New session…", format!("{claude} --bg [--name …] [-- prompt]"), Some(Action::New)));
    let ended = if cx.prefs.show_ended { "Hide ended sessions" } else { "Show ended sessions" };
    rows.push(MenuRow::new(
        "e",
        ended,
        "remembered; a search always includes them",
        Some(Action::ToggleEnded),
    ));
    rows.push(MenuRow::new("r", "Refresh", "Read the sessions again now", Some(Action::Refresh)));
    rows.push(MenuRow::new("?", "Keys", "Every key on the board (F1)", Some(Action::Help)));
    rows.push(MenuRow::new("q", "Quit", "Leave the board; sessions keep running", Some(Action::Quit)));
    rows
}

/// The open menu.
#[derive(Clone, Debug, PartialEq)]
pub struct ActionMenu {
    /// The session's title, or empty.
    pub title: String,
    pub rows: Vec<MenuRow>,
    pub sel: usize,
}

impl ActionMenu {
    /// The menu for the session under `cursor`.
    pub fn new(cx: &Ctx, cursor: Option<&Cursor>) -> ActionMenu {
        let s = match cursor {
            Some(Cursor::Session(id)) => cx.snap.get(id),
            _ => None,
        };
        ActionMenu { title: s.map(|s| s.title.clone()).unwrap_or_default(), rows: menu_rows(s, cx), sel: 0 }
    }

    /// ↑/↓ (Ctrl+P/N) choose, Enter runs the chosen row, a row's letter runs it, Esc or Ctrl+K
    /// closes. A refused row stays open, its reason shown.
    pub fn handle_key(&mut self, key: KeyEvent) -> Reply {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let n = self.rows.len();
        match key.code {
            KeyCode::Esc => Reply::Close,
            KeyCode::Char('k') if ctrl => Reply::Close,
            KeyCode::Up => self.go(self.sel.saturating_sub(1)),
            KeyCode::Char('p') if ctrl => self.go(self.sel.saturating_sub(1)),
            KeyCode::Down => self.go((self.sel + 1).min(n.saturating_sub(1))),
            KeyCode::Char('n') if ctrl => self.go((self.sel + 1).min(n.saturating_sub(1))),
            KeyCode::Home => self.go(0),
            KeyCode::End => self.go(n.saturating_sub(1)),
            KeyCode::Enter => self.pick(self.sel),
            KeyCode::Char(c) if !ctrl => {
                let k = c.to_string();
                match self.rows.iter().position(|r| r.key == k) {
                    Some(i) => self.pick(i),
                    None => Reply::Keep,
                }
            }
            _ => Reply::Keep,
        }
    }

    fn go(&mut self, i: usize) -> Reply {
        self.sel = i;
        Reply::Keep
    }

    fn pick(&mut self, i: usize) -> Reply {
        self.sel = i;
        match self.rows.get(i).and_then(|r| r.action.clone()) {
            Some(a) => Reply::Do(a),
            None => Reply::Keep,
        }
    }

    /// Draw over `area`: a popup of rows (key, label, command or reason), and under them the
    /// chosen row's detail in full.
    pub fn draw(&self, buf: &mut Buffer, area: Rect, t: &Theme) {
        let w = 78u16.min(area.width.saturating_sub(4));
        let text_w = usize::from(w.saturating_sub(6)).max(1);
        let sel = self.rows.get(self.sel);
        let detail = sel.map(|r| wrap(&r.detail, text_w)).unwrap_or_default();
        let detail_h = detail.len().min(4) as u16;
        let h = self.rows.len() as u16 + detail_h + 5;
        let mut title = vec![Span::styled(" Actions ", t.bold(t.accent))];
        if !self.title.is_empty() {
            title.push(Span::styled(format!("· {} ", trunc(&self.title, usize::from(w / 2))), fg(t.dim)));
        }
        let footer = Line::from(Span::styled(" ↑↓ choose · ↵ or a key runs it · Esc closes ", fg(t.faint)));
        let inner = popup(buf, t, area, w, h, t.accent, Line::from(title), footer);
        for (i, r) in self.rows.iter().enumerate() {
            let y = inner.y + 1 + i as u16;
            if y >= inner.bottom() {
                return;
            }
            let chosen = i == self.sel;
            let bg = if chosen { t.sel } else { t.raised };
            fill(buf, Rect::new(inner.x, y, inner.width, 1), Style::new().bg(bg));
            let ok = r.allowed();
            let cap = if ok {
                Style::new().fg(t.bright).bg(t.keycap).add_modifier(Modifier::BOLD)
            } else {
                Style::new().fg(t.faint).bg(bg)
            };
            let name = match (ok, chosen) {
                (false, _) => fg(t.faint).bg(bg),
                (true, true) => t.bold(t.bright).bg(bg),
                (true, false) => fg(t.fg).bg(bg),
            };
            let left = [
                Span::styled(if chosen { "▌" } else { " " }, fg(t.accent).bg(bg)),
                Span::styled(" ", Style::new().bg(bg)),
                Span::styled(format!(" {} ", r.key), cap),
                Span::styled("  ", Style::new().bg(bg)),
                Span::styled(r.label.clone(), name),
            ];
            let lx = put(buf, inner.x, y, &left, inner.width);
            let room = inner.right().saturating_sub(lx + 3);
            let dstyle = if ok { fg(t.dim).bg(bg) } else { fg(t.faint).bg(bg).add_modifier(Modifier::ITALIC) };
            // The title is in the popup's; the row says the rest.
            let prefix = format!("“{}” ", self.title);
            let d = trunc(r.detail.strip_prefix(&prefix).unwrap_or(&r.detail), usize::from(room));
            put_right(buf, lx + 2, y, room, &[Span::styled(d, dstyle)]);
        }
        let y0 = inner.y + 2 + self.rows.len() as u16;
        if let Some(r) = sel {
            let style = if r.allowed() { fg(t.code) } else { fg(t.warn) };
            for (k, line) in detail.iter().take(usize::from(detail_h)).enumerate() {
                let y = y0 + k as u16;
                if y >= inner.bottom() {
                    break;
                }
                let lead = if k == 0 { if r.allowed() { "$ " } else { "✗ " } } else { "  " };
                let x = put_str(buf, inner.x + 2, y, lead, fg(t.faint).bg(t.raised), 2);
                put_str(buf, x, y, line, style.bg(t.raised), inner.width.saturating_sub(4));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{Snapshot, State};
    use crate::store::Prefs;

    fn cx<'a>(snap: &'a Snapshot, theme: &'a Theme) -> Ctx<'a> {
        Ctx {
            snap,
            theme,
            now: 1000.0,
            home: "/home/u",
            scratch: "/tmp",
            days: 30,
            prefs: Prefs::default(),
            tick: 0,
            loaded: true,
            claude: &CLAUDE[..],
        }
    }

    static CLAUDE: std::sync::LazyLock<Vec<String>> = std::sync::LazyLock::new(|| vec!["claude".into()]);

    fn snap() -> Snapshot {
        let mut job = Session::new("a1", "Fix it", "/home/u/api", "/home/u/api", State::NeedsYou, 900.0);
        job.job = Some("a1b2c3d4".into());
        let elsewhere = Session::new("w1", "Copy review", "/home/u/web", "/home/u/web", State::Elsewhere, 900.0);
        let ended = Session::new("e1", "Old one", "/", "/", State::Ended, 100.0);
        Snapshot { now: 1000.0, generation: 1, sessions: vec![job, elsewhere, ended] }
    }

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    #[test]
    fn rows_show_the_command_or_the_refusal() {
        let (s, t) = (snap(), Theme::default());
        let cx = cx(&s, &t);
        let m = ActionMenu::new(&cx, Some(&Cursor::Session("a1".into())));
        assert_eq!(m.rows[0].detail, "claude attach a1b2c3d4");
        assert_eq!(m.rows[1].detail, "claude stop a1b2c3d4");
        assert_eq!(m.rows[2].detail, "claude rm a1b2c3d4");
        assert!(m.rows.iter().all(MenuRow::allowed));

        let m = ActionMenu::new(&cx, Some(&Cursor::Session("w1".into())));
        assert!(m.rows[..3].iter().all(|r| !r.allowed() && r.detail.contains("another terminal")));

        let m = ActionMenu::new(&cx, Some(&Cursor::Session("e1".into())));
        assert_eq!(m.rows[0].detail, "claude --bg --resume e1, then claude attach");
        assert!(m.rows[1].detail.contains("already ended"));
        assert!(m.rows[2].detail.contains("after 30 days"));

        let m = ActionMenu::new(&cx, None);
        assert!(!m.rows[0].allowed());
        assert_eq!(m.rows[3].action, Some(Action::New));
    }

    #[test]
    fn keys_choose_run_and_close() {
        let (s, t) = (snap(), Theme::default());
        let cx = cx(&s, &t);
        let mut m = ActionMenu::new(&cx, Some(&Cursor::Session("a1".into())));
        assert_eq!(m.handle_key(key(KeyCode::Down)), Reply::Keep);
        assert_eq!(m.handle_key(key(KeyCode::Enter)), Reply::Do(Action::Stop("a1".into())));
        assert_eq!(m.handle_key(key(KeyCode::Char('x'))), Reply::Do(Action::Delete("a1".into())));
        assert_eq!(m.handle_key(key(KeyCode::Char('e'))), Reply::Do(Action::ToggleEnded));
        assert_eq!(m.handle_key(key(KeyCode::Esc)), Reply::Close);
        assert_eq!(m.handle_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL)), Reply::Close);

        let mut m = ActionMenu::new(&cx, Some(&Cursor::Session("w1".into())));
        assert_eq!(m.handle_key(key(KeyCode::Enter)), Reply::Keep, "a refused row stays open");
        assert_eq!(m.handle_key(key(KeyCode::Char('s'))), Reply::Keep);
        assert_eq!(m.sel, 1);
    }

    #[test]
    fn it_draws_at_any_size() {
        let (s, t) = (snap(), Theme::default());
        let cx = cx(&s, &t);
        let m = ActionMenu::new(&cx, Some(&Cursor::Session("e1".into())));
        for (w, h) in [(1, 1), (20, 6), (80, 24), (160, 45)] {
            let mut buf = Buffer::empty(Rect::new(0, 0, w, h));
            let a = buf.area;
            m.draw(&mut buf, a, &t);
        }
    }
}
