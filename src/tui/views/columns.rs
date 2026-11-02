//! Columns, the default view: Miller columns, projects › sessions › conversation
//! (`mockups/rs/columns`).
//!
//! - **Projects** (left): a pinned virtual "Needs you" entry, then every project in the board's
//!   order with a needs-you count or a session count, and a legend of all sessions by state.
//! - **Sessions** (middle): the entry's sessions, two lines each (title and age; state and what
//!   it needs or is doing), ended ones under an ENDED divider.
//! - **Conversation** (right): the session's title and state, the metadata strip (`c`), your ask
//!   and Claude's reply as bubbles, and what it needs from you. Scrolls, with its own focus.
//!
//! ←/→ move between the columns, ↑/↓ (PgUp/PgDn, Home/End) within one; Enter on a project moves
//! into it, on a session jumps in. Three columns from 110 cells wide, then a sliding pair, then
//! one. The selection is the shared cursor (a session id); the column-1 entry and each entry's
//! last session are remembered here, by key, so a refresh that reorders rows never moves them.

mod lists;
mod preview;
mod work;

use super::board_rows;
use crate::actions::Cursor;
use crate::facts::{Session, State};
use crate::tui::app::{Action, Ctx, View, ViewId};
use crate::tui::theme::{Theme, fg};
use crate::tui::widgets::{
    self, ago, fit, glyph, key_hints, pill, project_name, put, short_path, spans_width, view_tabs, vline,
};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::Span;
use std::collections::HashMap;

/// What column 1 has selected.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Entry {
    /// The pinned virtual entry: every session that needs you, newest first.
    NeedsYou,
    /// A project, by path.
    Project(String),
}

/// Which column has the keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Focus {
    Projects,
    #[default]
    Sessions,
    Preview,
}

/// The snapshot as Columns shows it: the needs-you list and the projects with their sessions,
/// all in the board's order. Built fresh for every draw and key.
struct Board<'a> {
    needs: Vec<&'a Session>,
    projects: Vec<(String, Vec<&'a Session>)>,
}

impl<'a> Board<'a> {
    fn build(cx: &Ctx<'a>) -> Board<'a> {
        let mut projects: Vec<(String, Vec<&'a Session>)> = Vec::new();
        for s in board_rows(cx) {
            match projects.last_mut() {
                Some((p, ss)) if *p == s.project => ss.push(s),
                _ => projects.push((s.project.clone(), vec![s])),
            }
        }
        let mut needs: Vec<&'a Session> =
            projects.iter().flat_map(|(_, ss)| ss.iter().copied()).filter(|s| s.state == State::NeedsYou).collect();
        needs.sort_by(|a, b| b.last.total_cmp(&a.last));
        Board { needs, projects }
    }

    fn entries(&self) -> Vec<Entry> {
        std::iter::once(Entry::NeedsYou).chain(self.projects.iter().map(|(p, _)| Entry::Project(p.clone()))).collect()
    }

    fn has(&self, e: &Entry) -> bool {
        match e {
            Entry::NeedsYou => true,
            Entry::Project(p) => self.projects.iter().any(|(q, _)| q == p),
        }
    }

    fn list(&self, e: &Entry) -> &[&'a Session] {
        match e {
            Entry::NeedsYou => &self.needs,
            Entry::Project(p) => self.projects.iter().find(|(q, _)| q == p).map_or(&[], |(_, ss)| ss.as_slice()),
        }
    }
}

#[derive(Default)]
pub struct Columns {
    focus: Focus,
    /// Column 1's selection; `None` until the first draw picks one.
    entry: Option<Entry>,
    /// The session last selected under each entry.
    remembered: HashMap<Entry, String>,
    /// First visible row of the projects list and of the sessions list.
    projects_top: usize,
    sessions_top: u16,
    /// Rows of the sessions list on screen, for PgUp/PgDn.
    sessions_page: u16,
    /// The conversation's scroll, the session it belongs to, and its limits at the last draw.
    scroll: u16,
    scroll_for: Option<String>,
    max_scroll: u16,
    preview_page: u16,
}

impl Columns {
    /// Line the view's state up with the shared cursor and the snapshot as it is now: pick the
    /// column-1 entry the cursor's session belongs to (keeping the current one while it holds
    /// the session), and put the cursor on a session of that entry. Returns the selected session.
    fn sync<'a>(&mut self, b: &Board<'a>, cx: &Ctx<'a>, cursor: &mut Option<Cursor>) -> Option<&'a Session> {
        let current = self.entry.clone().filter(|e| b.has(e));
        let wanted = match cursor.as_ref() {
            Some(Cursor::Session(id)) => {
                if let Some(e) = current.as_ref().filter(|e| b.list(e).iter().any(|s| s.session_id == *id)) {
                    Some(e.clone())
                } else if let Some(s) = cx.snap.get(id) {
                    let project = Entry::Project(s.project.clone());
                    if self.entry.is_none() && b.needs.iter().any(|n| n.session_id == *id) {
                        Some(Entry::NeedsYou)
                    } else {
                        b.has(&project).then_some(project)
                    }
                } else {
                    None
                }
            }
            Some(Cursor::Project(p)) => Some(Entry::Project(p.clone())).filter(|e| b.has(e)),
            None => None,
        };
        let entry = wanted.or(current).unwrap_or_else(|| match b.projects.first() {
            Some((p, _)) if b.needs.is_empty() => Entry::Project(p.clone()),
            _ => Entry::NeedsYou,
        });
        let list = b.list(&entry);
        let by_id = |id: &str| list.iter().copied().find(|s| s.session_id == id);
        let chosen = match cursor.as_ref() {
            Some(Cursor::Session(id)) => by_id(id),
            _ => None,
        }
        .or_else(|| self.remembered.get(&entry).and_then(|id| by_id(id)))
        .or_else(|| list.first().copied());
        if let Some(s) = chosen {
            *cursor = Some(Cursor::Session(s.session_id.clone()));
            self.remembered.insert(entry.clone(), s.session_id.clone());
        }
        // Nothing to select (an empty "Needs you", or no sessions at all): the cursor is left
        // alone, so a selection survives a loading or empty snapshot.
        self.entry = Some(entry);
        if chosen.is_none() && self.focus != Focus::Projects {
            self.focus = if b.projects.is_empty() { Focus::Sessions } else { Focus::Projects };
        }
        chosen
    }

    fn entry(&self) -> Entry {
        self.entry.clone().unwrap_or(Entry::NeedsYou)
    }

    /// Select column-1 entry `e`: its remembered session, else its first; nothing if it's empty.
    fn select_entry(&mut self, b: &Board, e: Entry, cursor: &mut Option<Cursor>) {
        let list = b.list(&e);
        let id = self
            .remembered
            .get(&e)
            .filter(|id| list.iter().any(|s| s.session_id == **id))
            .cloned()
            .or_else(|| list.first().map(|s| s.session_id.clone()));
        *cursor = id.map(Cursor::Session);
        self.entry = Some(e);
    }

    /// The columns shown at width `w`, each with its width, left to right.
    fn layout(&self, w: u16) -> Vec<(Focus, u16)> {
        if w >= 110 {
            let c1 = (w * 15 / 100).clamp(22, 28);
            let c2 = (w * 30 / 100).clamp(36, 52);
            vec![(Focus::Projects, c1), (Focus::Sessions, c2), (Focus::Preview, w - c1 - c2 - 2)]
        } else if w >= 64 {
            if self.focus == Focus::Projects {
                vec![(Focus::Projects, 26), (Focus::Sessions, w - 27)]
            } else {
                let c2 = (w * 38 / 100).clamp(26, 34);
                vec![(Focus::Sessions, c2), (Focus::Preview, w - c2 - 1)]
            }
        } else {
            vec![(self.focus, w)]
        }
    }

    fn move_by(&mut self, b: &Board, cursor: &mut Option<Cursor>, sel: Option<&Session>, code: KeyCode) {
        let delta = |page: isize| -> isize {
            match code {
                KeyCode::Up => -1,
                KeyCode::Down => 1,
                KeyCode::PageUp => -page.max(1),
                KeyCode::PageDown => page.max(1),
                KeyCode::Home => isize::MIN / 2,
                _ => isize::MAX / 2,
            }
        };
        let step = |i: usize, n: usize, d: isize| (i as isize).saturating_add(d).clamp(0, n as isize - 1) as usize;
        match self.focus {
            Focus::Projects => {
                let entries = b.entries();
                let here = entries.iter().position(|e| *e == self.entry()).unwrap_or(0);
                let to = step(here, entries.len(), delta(10));
                if to != here {
                    self.select_entry(b, entries[to].clone(), cursor);
                }
            }
            Focus::Sessions => {
                let list = b.list(&self.entry());
                let Some(here) = sel.and_then(|s| list.iter().position(|t| t.session_id == s.session_id)) else { return };
                let to = step(here, list.len(), delta((self.sessions_page / 3) as isize));
                *cursor = Some(Cursor::Session(list[to].session_id.clone()));
            }
            Focus::Preview => {
                let page = i32::from(self.preview_page.saturating_sub(2).max(1));
                let to = match code {
                    KeyCode::Up => i32::from(self.scroll) - 1,
                    KeyCode::Down => i32::from(self.scroll) + 1,
                    KeyCode::PageUp => i32::from(self.scroll) - page,
                    KeyCode::PageDown => i32::from(self.scroll) + page,
                    KeyCode::Home => 0,
                    _ => i32::from(self.max_scroll),
                };
                self.scroll = to.clamp(0, i32::from(self.max_scroll)) as u16;
            }
        }
    }

    fn draw_preview(&mut self, buf: &mut Buffer, r: Rect, cx: &Ctx, b: &Board, sel: Option<&Session>, focused: bool) {
        let t = cx.theme;
        let Some(s) = sel else {
            let (head, note) = match (cx.loaded, b.projects.is_empty()) {
                (false, _) => ("Reading sessions…", ""),
                (true, true) => ("No sessions", "n starts one in the background."),
                (true, false) => ("Nothing selected", "Pick a project or a session."),
            };
            put(buf, r.x + 2, r.y + 1, &[Span::styled(head, t.bold(t.dim))], r.width.saturating_sub(3));
            put(buf, r.x + 2, r.y + 2, &[Span::styled(note, fg(t.faint))], r.width.saturating_sub(3));
            return;
        };
        if self.scroll_for.as_deref() != Some(&s.session_id) {
            self.scroll_for = Some(s.session_id.clone());
            self.scroll = 0;
        }
        // Measure on a one-row buffer (drawing clips), then draw into one just tall enough.
        let mut probe = Buffer::empty(Rect::new(0, 0, r.width, 1));
        let h = preview::render(&mut probe, r.width, cx, s).max(1);
        let mut scratch = Buffer::empty(Rect::new(0, 0, r.width, h));
        widgets::fill(&mut scratch, Rect::new(0, 0, r.width, h), t.text());
        preview::render(&mut scratch, r.width, cx, s);

        self.preview_page = r.height;
        self.max_scroll = h.saturating_sub(r.height);
        self.scroll = self.scroll.min(self.max_scroll);
        for yy in 0..r.height.min(h - self.scroll) {
            for xx in 0..r.width {
                let src = scratch[(xx, yy + self.scroll)].clone();
                if let Some(c) = buf.cell_mut((r.x + xx, r.y + yy)) {
                    *c = src;
                }
            }
        }
        widgets::scrollbar(buf, t, r, usize::from(h), usize::from(r.height), usize::from(self.scroll), focused);
    }

    fn draw_header(&self, buf: &mut Buffer, r: Rect, cx: &Ctx, sel: Option<&Session>, hidden_parent: bool) {
        let t = cx.theme;
        widgets::fill(buf, r, Style::new().bg(t.bar));
        let mut x = r.x;
        if r.width >= 100 {
            x = put(buf, x, r.y, &[pill("session-board", t.needs_ink, t.accent)], r.width) + 1;
        }
        x = put(buf, x, r.y, &view_tabs(t, ViewId::Columns, t.bar), r.right().saturating_sub(x));

        let all = &cx.snap.sessions;
        let needs = all.iter().filter(|s| s.state == State::NeedsYou).count();
        let mut right: Vec<Span> = Vec::new();
        if r.width >= 120 {
            let running = all.iter().filter(|s| s.state == State::Running).count();
            let shown = cx.visible().count();
            right.push(Span::styled(format!("{running} running"), fg(t.running).bg(t.bar)));
            right.push(Span::styled("  ·  ", fg(t.faint).bg(t.bar)));
            right.push(Span::styled(format!("{shown} sessions  "), fg(t.dim).bg(t.bar)));
        }
        if needs > 0 {
            right.push(pill(&format!("◆ {needs} need{} you", if needs == 1 { "s" } else { "" }), t.needs_ink, t.needs));
        }
        let rw = spans_width(&right) as u16;
        widgets::put_right(buf, r.x, r.y, r.width, &right);

        // The breadcrumb: entry › session › conversation, the focused column's part bright.
        let room = r.right().saturating_sub(rw + 2).saturating_sub(x + 2);
        if room < 8 {
            return;
        }
        let part = |on: bool| if on { t.bold(t.bright).bg(t.bar) } else { fg(t.dim).bg(t.bar) };
        let sep = || Span::styled("  ›  ", fg(t.faint).bg(t.bar));
        let mut crumbs: Vec<Span> = Vec::new();
        if hidden_parent {
            crumbs.push(Span::styled("‹ ", t.bold(t.accent).bg(t.bar)));
        }
        match self.entry() {
            Entry::NeedsYou => {
                crumbs.push(Span::styled("◆ ", fg(t.needs).bg(t.bar)));
                crumbs.push(Span::styled("Needs you", part(self.focus == Focus::Projects)));
            }
            Entry::Project(p) => {
                crumbs.push(Span::styled(project_name(&p, cx.home, cx.scratch), part(self.focus == Focus::Projects)));
                if room > 90 && p != cx.home {
                    crumbs.push(Span::styled(format!("  {}", short_path(&p, cx.home)), fg(t.faint).bg(t.bar)));
                }
            }
        }
        if let Some(s) = sel {
            crumbs.push(sep());
            crumbs.push(Span::styled(format!("{} ", glyph(s.state)), fg(t.state(s.state)).bg(t.bar)));
            crumbs.push(Span::styled(s.title.clone(), part(self.focus != Focus::Projects)));
            if self.focus == Focus::Preview {
                crumbs.push(sep());
                crumbs.push(Span::styled("conversation", part(true)));
            }
        }
        put(buf, x + 2, r.y, &fit(&crumbs, usize::from(room)), room);
    }

    fn draw_footer(&self, buf: &mut Buffer, r: Rect, cx: &Ctx, b: &Board, sel: Option<&Session>) {
        let t = cx.theme;
        widgets::fill(buf, r, Style::new().bg(t.bar));
        let (mode, pos) = match self.focus {
            Focus::Projects => {
                let entries = b.entries();
                let i = entries.iter().position(|e| *e == self.entry()).unwrap_or(0);
                ("PROJECTS", format!(" {}/{} ", i + 1, entries.len()))
            }
            f => {
                let list = b.list(&self.entry());
                let i = sel.and_then(|s| list.iter().position(|x| x.session_id == s.session_id)).map_or(0, |i| i + 1);
                (match f {
                    Focus::Sessions => "SESSIONS",
                    _ if r.width >= 100 => "CONVERSATION",
                    _ => "PREVIEW",
                }, format!(" {i}/{} ", list.len()))
            }
        };
        let lead = [pill(mode, t.needs_ink, t.accent), Span::styled(pos, fg(t.fg).bg(t.keycap)), Span::styled("  ", fg(t.dim).bg(t.bar))];
        let x = put(buf, r.x, r.y, &lead, r.width);

        let wide = r.width >= 100;
        let ended = if cx.prefs.show_ended { "hide ended" } else { "show ended" };
        let enter = match (self.focus, sel) {
            (Focus::Projects, _) => "open",
            (_, Some(s)) if s.state == State::Ended => "resume",
            _ if wide => "jump in",
            _ => "jump",
        };
        let updown = if self.focus == Focus::Preview { "scroll" } else { "move" };
        // (rank, key, label): the most useful are kept when the bar is short; shown in order.
        let all: [(u8, &str, &str); 11] = [
            (0, "←→", if wide { "column" } else { "col" }),
            (2, "↑↓", updown),
            (1, "↵", enter),
            (4, "s", "stop"),
            (5, "x", if wide { "delete" } else { "del" }),
            (6, "n", "new"),
            (7, "/", "search"),
            (8, "e", ended),
            (9, "c", "details"),
            (10, "Tab", "view"),
            (3, "q", "quit"),
        ];
        let room = usize::from(r.right().saturating_sub(x + 1));
        let mut keep = [false; 11];
        let mut used = 0;
        for rank in 0..=10u8 {
            if let Some(i) = all.iter().position(|k| k.0 == rank) {
                let w = spans_width(&key_hints(t, &[(all[i].1, all[i].2)], t.bar));
                if used + w <= room {
                    keep[i] = true;
                    used += w;
                }
            }
        }
        let keys: Vec<(&str, &str)> = all.iter().zip(keep).filter(|(_, k)| *k).map(|(k, _)| (k.1, k.2)).collect();
        put(buf, x, r.y, &key_hints(t, &keys, t.bar), r.right().saturating_sub(x));
    }
}

impl View for Columns {
    fn draw(&mut self, frame: &mut Frame, area: Rect, cx: &Ctx, cursor: &mut Option<Cursor>) {
        let b = Board::build(cx);
        let sel = self.sync(&b, cx, cursor);
        let buf = frame.buffer_mut();
        let t = cx.theme;
        widgets::fill(buf, area, t.text());

        let body = Rect::new(area.x, area.y + 1, area.width, area.height.saturating_sub(2));
        let cols = self.layout(body.width);
        let mut x = body.x;
        for (k, &(col, w)) in cols.iter().enumerate() {
            let r = Rect::new(x, body.y, w, body.height);
            let focused = self.focus == col;
            match col {
                Focus::Projects => lists::projects(self, buf, r, cx, &b, focused),
                Focus::Sessions => lists::sessions(self, buf, r, cx, &b, sel, focused),
                Focus::Preview => self.draw_preview(buf, r, cx, &b, sel, focused),
            }
            x += w;
            if k + 1 < cols.len() {
                vline(buf, x, body.y, body.height, "│", fg(t.sep).bg(t.bg));
                x += 1;
            }
        }
        let hidden_parent = cols.first().is_some_and(|c| c.0 != Focus::Projects);
        self.draw_header(buf, Rect::new(area.x, area.y, area.width, 1), cx, sel, hidden_parent);
        self.draw_footer(buf, Rect::new(area.x, area.bottom() - 1, area.width, 1), cx, &b, sel);
    }

    fn handle_key(&mut self, key: KeyEvent, cx: &Ctx, cursor: &mut Option<Cursor>) -> Option<Action> {
        if key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
            return None;
        }
        let b = Board::build(cx);
        let sel = self.sync(&b, cx, cursor);
        let has_rows = !b.list(&self.entry()).is_empty();
        match key.code {
            KeyCode::Left => {
                self.focus = match self.focus {
                    Focus::Preview => Focus::Sessions,
                    _ => Focus::Projects,
                };
            }
            KeyCode::Right => {
                self.focus = match self.focus {
                    Focus::Projects if has_rows => Focus::Sessions,
                    Focus::Sessions if sel.is_some() => Focus::Preview,
                    f => f,
                };
            }
            KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown | KeyCode::Home | KeyCode::End => {
                self.move_by(&b, cursor, sel, key.code);
            }
            KeyCode::Enter if self.focus == Focus::Projects => {
                if has_rows {
                    self.focus = Focus::Sessions;
                }
            }
            KeyCode::Char('c') => return Some(Action::ToggleColumns),
            KeyCode::Char('f') => return Some(Action::ToggleFiles),
            _ => return None,
        }
        Some(Action::Handled)
    }
}

/// A selected row's fill: lit in the focused column (amber for needs-you) with a bar at its
/// left edge, quiet elsewhere.
fn sel_bar(buf: &mut Buffer, t: &Theme, r: Rect, focused: bool, hot: bool) {
    let bg = match (focused, hot) {
        (true, true) => t.needs_sel,
        (true, false) => t.sel,
        _ => t.sel_blur,
    };
    widgets::fill(buf, r, Style::new().bg(bg));
    if focused {
        vline(buf, r.x, r.y, r.height, "▌", fg(if hot { t.needs } else { t.accent }).bg(bg));
    }
}

/// "5m ago", or "just now".
fn ago_text(s: &Session, now: f64) -> String {
    match ago(s.last, now).as_str() {
        "now" => "just now".into(),
        a => format!("{a} ago"),
    }
}
