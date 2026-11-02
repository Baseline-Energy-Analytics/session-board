//! Timeline: each session's activity over the last 24 hours or 7 days, Gantt-style
//! (`mockups/rs/timeline`), drawn from the real activity spans and `waiting_since` in the facts.
//!
//! Rows are the board's (projects in the board's order, a header row each, sessions by state
//! then recency). Bars sit on a local-time axis with ticks, night shading and a "now" line. A
//! needs-you row is tinted, with a ◆ where it began waiting, a dashed line on to now and a
//! "waiting 42m" pill. A row with nothing in range says how long ago it was last active. Above
//! the axis, a sparkline counts how many sessions were active at each moment (every session,
//! whether or not ended ones are shown). Below, the selected session's details.
//!
//! Keys: ↑/↓, PgUp/PgDn, Home/End move; `t` toggles 24h/7d, `1` and `7` pick one. Enter and the
//! rest are the app's.

use super::{board_rows, resolve, step};
use crate::actions::Cursor;
use crate::facts::{Session, State};
use crate::tui::app::{Action, Ctx, View, ViewId};
use crate::tui::theme::{Theme, fg};
use crate::tui::widgets::{
    self, ago, context_gauge, fill, glyph, key_hints, model_name, panel, pill, project_name, put, put_right, put_str,
    short_path, spinner, tokens, trunc, view_tabs, width, wrap,
};
use chrono::{DateTime, Local, TimeZone, Timelike};
use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Below this width: no state column, ages in the label column, a stacked detail panel.
const COMPACT_BELOW: u16 = 110;
/// The state column's width.
const GUTTER: u16 = 16;
/// The shortest a span is drawn: one entry on its own is about a minute of work.
const MIN_SPAN: f64 = 60.0;
/// Local hours outside `DAY` are shaded as night.
const DAY: std::ops::Range<u32> = 7..22;

/// The time axis' extent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Range {
    #[default]
    Day,
    Week,
}

impl Range {
    fn secs(self) -> f64 {
        match self {
            Range::Day => 86_400.0,
            Range::Week => 7.0 * 86_400.0,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Range::Day => "last 24 hours",
            Range::Week => "last 7 days",
        }
    }
}

#[derive(Default)]
pub struct Timeline {
    pub range: Range,
    /// The first display row shown (group headers count).
    scroll: usize,
    /// Rows the body showed last frame, for PgUp/PgDn.
    page: usize,
}

impl View for Timeline {
    fn draw(&mut self, frame: &mut Frame, area: Rect, cx: &Ctx, cursor: &mut Option<Cursor>) {
        let rows = board_rows(cx);
        let sel = resolve(&rows, cursor);
        let selected = sel.map(|i| rows[i]);
        let display = layout(&rows);
        let buf = frame.buffer_mut();
        let t = cx.theme;
        fill(buf, area, t.text());

        // Header, footer, detail panel, and the timeline in between.
        let compact = area.width < COMPACT_BELOW;
        let header = Rect::new(area.x, area.y, area.width, 1);
        let footer = Rect::new(area.x, area.bottom() - 1, area.width, 1);
        let want = display.len() as u16 + 3;
        let spare = area.height.saturating_sub(2 + want);
        let (lo, hi) = if compact { (13, 18) } else { (12, 13) };
        // Keep at least the axis and four rows of timeline; drop the panel if it gets too thin.
        let room = area.height.saturating_sub(2 + 3 + 4);
        let mut detail_h = spare.clamp(lo, hi).min(room);
        if detail_h < 6 || selected.is_none() {
            detail_h = 0;
        }
        let detail = Rect::new(area.x, footer.y - detail_h, area.width, detail_h);
        let body = Rect::new(area.x, area.y + 1, area.width, detail.y.saturating_sub(area.y + 1));

        draw_header(buf, header, cx, self.range);
        let geo = Geo::new(body, cx.now, self.range);
        self.page = usize::from(body.height.saturating_sub(3)).max(1);
        draw_body(buf, body, cx, &geo, &display, selected, &mut self.scroll);
        if let Some(s) = selected.filter(|_| detail_h > 0) {
            draw_detail(buf, detail, cx, s);
        }
        draw_footer(buf, footer, t);
    }

    fn handle_key(&mut self, key: KeyEvent, cx: &Ctx, cursor: &mut Option<Cursor>) -> Option<Action> {
        if key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) {
            return None;
        }
        let page = self.page.max(1) as isize;
        let delta = match key.code {
            KeyCode::Char('t') => {
                self.range = if self.range == Range::Day { Range::Week } else { Range::Day };
                return Some(Action::Handled);
            }
            KeyCode::Char('1') => {
                self.range = Range::Day;
                return Some(Action::Handled);
            }
            KeyCode::Char('7') => {
                self.range = Range::Week;
                return Some(Action::Handled);
            }
            KeyCode::Up => -1,
            KeyCode::Down => 1,
            KeyCode::PageUp => -page,
            KeyCode::PageDown => page,
            KeyCode::Home => isize::MIN / 2,
            KeyCode::End => isize::MAX / 2,
            _ => return None,
        };
        step(&board_rows(cx), cursor, delta);
        Some(Action::Handled)
    }
}

// --- rows ----------------------------------------------------------------------------------------

enum Row<'a> {
    /// A project's header and its (visible) sessions.
    Group { path: &'a str, members: Vec<&'a Session> },
    /// A session; `odd` for the alternate stripe within its project.
    Sess { s: &'a Session, odd: bool },
}

/// The board's rows with a header before each project.
fn layout<'a>(rows: &[&'a Session]) -> Vec<Row<'a>> {
    let mut out: Vec<Row<'a>> = Vec::new();
    let mut i = 0;
    while i < rows.len() {
        let path = rows[i].project.as_str();
        let members: Vec<&Session> = rows[i..].iter().take_while(|s| s.project == path).copied().collect();
        i += members.len();
        out.push(Row::Group { path, members: members.clone() });
        out.extend(members.into_iter().enumerate().map(|(k, s)| Row::Sess { s, odd: k % 2 == 1 }));
    }
    out
}

// --- the axis ------------------------------------------------------------------------------------

/// Where things go across the screen, and what time each track column stands for.
struct Geo {
    x0: u16,
    /// The label column's width.
    label_w: u16,
    /// The track's left edge and width in cells.
    tx: u16,
    tw: u16,
    /// The "now" line's column, just right of the track.
    now_x: u16,
    /// The state column (0 wide when compact).
    gx: u16,
    gw: u16,
    right: u16,
    /// The time at the track's left edge, and seconds per cell.
    t0: f64,
    cell: f64,
    compact: bool,
    range: Range,
    /// Per track column: shaded as night, carries a tick.
    night: Vec<bool>,
    tick: Vec<bool>,
    ticks: Vec<(i32, String, bool)>,
}

impl Geo {
    fn new(r: Rect, now: f64, range: Range) -> Geo {
        let compact = r.width < COMPACT_BELOW;
        let label_w = if compact { 27.min(r.width / 3) } else { 40 };
        let gw = if compact { 0 } else { GUTTER };
        let tx = r.x + label_w + 1;
        // label | space | track | now | (space | gutter) | scrollbar column
        let fixed = label_w + 1 + 1 + if gw > 0 { gw + 1 } else { 0 } + 1;
        let tw = r.width.saturating_sub(fixed).max(1);
        let now_x = tx + tw;
        let cell = range.secs() / f64::from(tw);
        let t0 = now - range.secs();
        let mut g = Geo {
            x0: r.x,
            label_w,
            tx,
            tw,
            now_x,
            gx: now_x + 2,
            gw,
            right: r.right(),
            t0,
            cell,
            compact,
            range,
            night: vec![],
            tick: vec![],
            ticks: vec![],
        };
        g.night = (0..tw).map(|c| is_night(g.mid(i32::from(c)))).collect();
        g.ticks = g.make_ticks();
        g.tick = (0..i32::from(tw)).map(|c| g.ticks.iter().any(|t| t.0 == c)).collect();
        g
    }

    /// The track column of time `t` (may be off the track).
    fn col(&self, t: f64) -> i32 {
        ((t - self.t0) / self.cell).floor() as i32
    }

    /// The time at the middle of column `c`.
    fn mid(&self, c: i32) -> f64 {
        self.t0 + (f64::from(c) + 0.5) * self.cell
    }

    /// Column `c`'s time interval.
    fn bounds(&self, c: usize) -> (f64, f64) {
        (self.t0 + c as f64 * self.cell, self.t0 + (c + 1) as f64 * self.cell)
    }

    fn on_track(&self, c: i32) -> bool {
        c >= 0 && c < i32::from(self.tw)
    }

    /// Ticks as (column, label, major): hours (midnight as the day's name) for 24h, days (and
    /// noon when there's room) for 7d, in local time.
    fn make_ticks(&self) -> Vec<(i32, String, bool)> {
        let mut out = vec![];
        let end = self.t0 + self.range.secs();
        let Some(start) = local(self.t0) else { return out };
        let Some(mut t) = start.with_minute(0).and_then(|d| d.with_second(0)) else { return out };
        let min_gap = 9.0;
        match self.range {
            Range::Day => {
                let step = [1u32, 2, 3, 4, 6, 12].into_iter().find(|h| f64::from(*h) * 3600.0 / self.cell >= min_gap).unwrap_or(12);
                while (t.timestamp() as f64) < end {
                    let ts = t.timestamp() as f64;
                    if ts >= self.t0 && t.hour() % step == 0 {
                        let major = t.hour() == 0;
                        let label = if major { t.format("%a").to_string() } else { t.format("%H:%M").to_string() };
                        out.push((self.col(ts), label, major));
                    }
                    t += chrono::Duration::hours(1);
                }
            }
            Range::Week => {
                let per_day = 86_400.0 / self.cell;
                let fmt = if per_day >= 10.0 { "%a %-d" } else { "%a" };
                let Some(mut d) = t.with_hour(0) else { return out };
                while (d.timestamp() as f64) < end {
                    let ts = d.timestamp() as f64;
                    if ts >= self.t0 {
                        out.push((self.col(ts), d.format(fmt).to_string(), true));
                    }
                    if per_day >= 24.0 {
                        let noon = (d + chrono::Duration::hours(12)).timestamp() as f64;
                        if noon >= self.t0 && noon < end {
                            out.push((self.col(noon), "12:00".into(), false));
                        }
                    }
                    // Across a DST change a day isn't 24h; step to the next local midnight.
                    let next = d + chrono::Duration::hours(26);
                    d = next.with_hour(0).unwrap_or(next);
                }
            }
        }
        out
    }
}

fn local(t: f64) -> Option<DateTime<Local>> {
    Local.timestamp_opt(t as i64, 0).earliest()
}

fn is_night(t: f64) -> bool {
    local(t).is_some_and(|d| !DAY.contains(&d.hour()))
}

/// How much of each track column a session's spans cover, 0..1. A running session's last span
/// runs on to now: it is working as we speak.
fn coverage(g: &Geo, s: &Session, now: f64) -> Vec<f64> {
    let mut cov = vec![0.0; usize::from(g.tw)];
    for (a, b) in spans(s, now) {
        let (c0, c1) = (g.col(a).max(0), g.col(b).min(i32::from(g.tw) - 1));
        for c in c0..=c1 {
            let (lo, hi) = g.bounds(c as usize);
            let overlap = (b.min(hi) - a.max(lo)).max(0.0);
            cov[c as usize] = (cov[c as usize] + overlap / g.cell).min(1.0);
        }
    }
    cov
}

/// A session's spans as (start, end), each at least [`MIN_SPAN`] long, a running one's last
/// span extended to now.
fn spans(s: &Session, now: f64) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = s.spans.iter().map(|p| (p.start, p.end.max(p.start + MIN_SPAN))).collect();
    if s.state == State::Running {
        match out.last_mut() {
            Some(last) => last.1 = last.1.max(now),
            None => out.push((s.last.min(now - MIN_SPAN), now)),
        }
    }
    out
}

/// When the session began waiting on you: a needs-you session (at `waiting_since`, else its last
/// activity) or a stale one whose state.json still says it needs you.
fn wait_start(s: &Session) -> Option<f64> {
    match s.state {
        State::NeedsYou => Some(s.waiting_since.unwrap_or(s.last)),
        State::Stale if s.was == Some(State::NeedsYou) => s.waiting_since,
        _ => None,
    }
}

/// The time of a session's latest known activity.
fn latest(s: &Session) -> f64 {
    s.spans.iter().map(|p| p.end).fold(s.last, f64::max)
}

// --- drawing helpers -----------------------------------------------------------------------------

/// Set one cell's symbol and foreground, if it's on the buffer.
fn set(buf: &mut Buffer, x: u16, y: u16, sym: &str, color: Color) {
    if let Some(c) = buf.cell_mut((x, y)) {
        c.set_symbol(sym).set_fg(color);
    }
}

fn set_bg(buf: &mut Buffer, x: u16, y: u16, color: Color) {
    if let Some(c) = buf.cell_mut((x, y)) {
        c.set_bg(color);
    }
}

fn bg_at(buf: &Buffer, x: u16, y: u16) -> Color {
    buf.cell((x, y)).map(|c| c.bg).unwrap_or_default()
}

/// A bar cell's glyph for the share of the cell that was active.
fn bar_glyph(f: f64) -> &'static str {
    if f >= 0.4 {
        "▆"
    } else if f >= 0.12 {
        "▄"
    } else {
        "▂"
    }
}

/// Where the "waiting" pill of width `w` starts on a track `tw` wide with the marker at `mark`:
/// right of the marker if it fits before now, else the place left of the marker that hides the
/// fewest cells of work (the nearest of those), so the bursts that led up to the wait show.
fn pill_x(cov: &[f64], mark: i32, w: i32, tw: i32) -> i32 {
    if mark + 2 + w <= tw {
        return mark + 2;
    }
    let hidden = |x: i32| (x..x + w).filter(|c| cov.get(*c as usize).is_some_and(|f| *f > 0.0)).count();
    (0..=mark - 1 - w).rev().min_by_key(|x| hidden(*x)).unwrap_or((mark - 1 - w).max(0))
}

/// A duration: `<1m`, `42m`, `1h 19m`, `2d 3h`.
fn duration(secs: f64) -> String {
    let m = (secs.max(0.0) / 60.0) as u64;
    match m {
        0 => "<1m".into(),
        1..=59 => format!("{m}m"),
        60..=1439 if m.is_multiple_of(60) => format!("{}h", m / 60),
        60..=1439 => format!("{}h {:02}m", m / 60, m % 60),
        _ if (m / 60).is_multiple_of(24) => format!("{}d", m / 1440),
        _ => format!("{}d {}h", m / 1440, (m / 60) % 24),
    }
}

fn bar_color(t: &Theme, s: State) -> Color {
    if s == State::NeedsYou { t.darken(t.needs, 0.72) } else { t.state(s) }
}

/// The night-shaded background of a track cell over `base`.
fn shade(t: &Theme, base: Color, night: bool) -> Color {
    if night { t.darken(base, 0.6) } else { base }
}

// --- header and footer ---------------------------------------------------------------------------

fn draw_header(buf: &mut Buffer, r: Rect, cx: &Ctx, range: Range) {
    let t = cx.theme;
    fill(buf, r, Style::new().bg(t.bar));
    let compact = r.width < COMPACT_BELOW;
    let mut x = put(buf, r.x, r.y, &[pill("session-board", t.needs_ink, t.accent)], r.width);
    x = put(buf, x + 1, r.y, &view_tabs(t, ViewId::Timeline, t.bar), r.right().saturating_sub(x + 1));
    x += 2;
    for (rg, label) in [(Range::Day, " 24h "), (Range::Week, " 7d ")] {
        let style = if rg == range {
            Style::new().fg(t.needs_ink).bg(t.dim).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(t.dim).bg(t.keycap)
        };
        x = put_str(buf, x, r.y, label, style, r.right().saturating_sub(x));
    }

    // Right: the needs-you count loudest, then the rest, then the clock.
    let count = |st: State| cx.snap.sessions.iter().filter(|s| s.state == st).count();
    let mut items: Vec<Span> = vec![];
    let n = count(State::NeedsYou);
    if n > 0 {
        items.push(pill(&format!("◆ {n} need{} you", if n == 1 { "s" } else { "" }), t.needs_ink, t.needs));
        items.push(Span::styled("  ", Style::new().bg(t.bar)));
    }
    let rest: &[(State, &str)] = if compact {
        &[(State::Running, "running")]
    } else {
        &[
            (State::Running, "running"),
            (State::Elsewhere, "elsewhere"),
            (State::Idle, "idle"),
            (State::Stale, "not running"),
            (State::Ended, "ended"),
        ]
    };
    for (st, label) in rest {
        let c = count(*st);
        if c > 0 {
            items.push(Span::styled(format!("{} {c} {label}  ", glyph(*st)), fg(t.state(*st)).bg(t.bar)));
        }
    }
    if !compact && let Some(d) = local(cx.now) {
        items.push(Span::styled(format!("{} ", d.format("%a %H:%M")), fg(t.dim).bg(t.bar)));
    }
    let x = x + 2;
    if x < r.right() {
        // Drop items from the end until they fit, rather than cutting the needs-you pill.
        while widgets::spans_width(&items) > usize::from(r.right() - x) && !items.is_empty() {
            items.pop();
        }
        put_right(buf, x, r.y, r.right() - x, &items);
    }
}

fn draw_footer(buf: &mut Buffer, r: Rect, t: &Theme) {
    fill(buf, r, Style::new().bg(t.bar));
    let keys: &[(&str, &str)] = if r.width < COMPACT_BELOW {
        &[("↵", "jump in"), ("↑↓", "move"), ("t", "24h/7d"), ("Tab", "view"), ("q", "quit")]
    } else {
        &[
            ("↵", "jump in"),
            ("↑↓", "move"),
            ("t", "24h/7d"),
            ("Tab", "view"),
            ("/", "search"),
            ("s", "stop"),
            ("x", "delete"),
            ("e", "ended"),
            ("F1", "help"),
            ("q", "quit"),
        ]
    };
    put(buf, r.x + 1, r.y, &key_hints(t, keys, t.bar), r.width.saturating_sub(2));
}

// --- the timeline --------------------------------------------------------------------------------

fn draw_body(buf: &mut Buffer, r: Rect, cx: &Ctx, g: &Geo, rows: &[Row], selected: Option<&Session>, scroll: &mut usize) {
    let t = cx.theme;
    if r.height == 0 {
        return;
    }
    let mut y = r.y;
    let label = |w: u16| w.saturating_sub(2);

    // Row 1: the activity sparkline, over every session (ended ones too, shown or not).
    let mut work = vec![0usize; usize::from(g.tw)];
    let mut wait = vec![false; usize::from(g.tw)];
    for s in &cx.snap.sessions {
        for (c, f) in coverage(g, s, cx.now).into_iter().enumerate() {
            if f > 0.0 {
                work[c] += 1;
            }
        }
        if s.state == State::NeedsYou
            && let Some(w) = wait_start(s)
        {
            for (c, slot) in wait.iter_mut().enumerate() {
                *slot |= w < g.bounds(c).1;
            }
        }
    }
    let peak = work.iter().copied().max().unwrap_or(0).max(1);
    let act = if g.compact { format!("activity · peak {peak}") } else { "activity · all sessions".into() };
    put_str(buf, g.x0 + 2, y, &act, fg(t.dim), label(g.label_w));
    const LEVELS: [&str; 9] = [" ", "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█"];
    let (lo, hi) = (t.darken(t.running, 0.45), t.mix(t.running, t.bright, 0.45));
    for c in 0..g.tw {
        let v = work[usize::from(c)];
        if v > 0 {
            let lvl = ((v as f64 / peak as f64) * 8.0).ceil().clamp(1.0, 8.0) as usize;
            set(buf, g.tx + c, y, LEVELS[lvl], t.mix(lo, hi, v as f32 / peak as f32));
        } else if wait[usize::from(c)] {
            set(buf, g.tx + c, y, "▁", t.darken(t.needs, 0.72));
        }
    }
    if g.gw > 0 {
        put_str(buf, g.gx, y, &format!("peak {peak}"), fg(t.dim), g.gw);
    }
    y += 1;
    if y >= r.bottom() {
        return;
    }

    // Row 2: the range and the tick labels. Row 3: the ruler.
    put_str(buf, g.x0 + 2, y, g.range.label(), t.bold(t.fg), label(g.label_w));
    let mut last_end = -1;
    for (c, text, major) in &g.ticks {
        let w = width(text) as i32;
        if *c <= last_end || *c < 0 || *c + w > i32::from(g.tw) - 3 {
            continue;
        }
        let style = if *major { t.bold(t.fg) } else { fg(t.dim) };
        put_str(buf, g.tx + *c as u16, y, text, style, w as u16);
        last_end = *c + w;
    }
    put_str(buf, g.now_x.saturating_sub(1), y, "now", t.bold(t.bright), 3);
    y += 1;
    if y >= r.bottom() {
        return;
    }
    for c in 0..g.tw {
        set(buf, g.tx + c, y, if g.tick[usize::from(c)] { "┬" } else { "─" }, t.sep);
    }
    set(buf, g.now_x, y, "┬", t.fg);
    if g.gw > 0 {
        put_str(buf, g.gx, y, "state", fg(t.dim), g.gw);
    }
    y += 1;

    // The rows, scrolled so the selection (and its project's header) stays in view.
    let avail = usize::from(r.bottom().saturating_sub(y));
    if avail == 0 {
        return;
    }
    let sel_row = selected.and_then(|sel| {
        rows.iter().position(|row| matches!(row, Row::Sess { s, .. } if s.session_id == sel.session_id))
    });
    if let Some(k) = sel_row {
        if k >= *scroll + avail {
            *scroll = k + 1 - avail;
        }
        let top = if k > 0 && matches!(rows[k - 1], Row::Group { .. }) { k - 1 } else { k };
        if top < *scroll {
            *scroll = top;
        }
    }
    *scroll = (*scroll).min(rows.len().saturating_sub(avail));
    let first_y = y;
    for (k, row) in rows.iter().enumerate().skip(*scroll).take(avail) {
        match row {
            Row::Group { path, members } => draw_group(buf, cx, g, y, path, members),
            Row::Sess { s, odd } => draw_session(buf, cx, g, y, s, Some(k) == sel_row, *odd),
        }
        y += 1;
    }
    if rows.is_empty() {
        let note = if cx.loaded { "No sessions to show" } else { "Reading sessions…" };
        put_str(buf, g.x0 + 2, y, note, fg(t.faint), label(g.label_w));
    }
    // The grid and the now line carry on below the last row.
    while y < r.bottom() {
        for c in 0..g.tw {
            if g.tick[usize::from(c)] {
                set(buf, g.tx + c, y, "┊", t.mix(t.bg, t.sep, 0.5));
            }
        }
        set(buf, g.now_x, y, "│", t.darken(t.fg, 0.5));
        y += 1;
    }
    let sb = Rect::new(g.right.saturating_sub(1), first_y, 1, r.bottom().saturating_sub(first_y));
    widgets::scrollbar(buf, t, sb, rows.len(), avail, *scroll, false);
}

fn draw_group(buf: &mut Buffer, cx: &Ctx, g: &Geo, y: u16, path: &str, members: &[&Session]) {
    let t = cx.theme;
    fill(buf, Rect::new(g.x0, y, g.right - g.x0, 1), Style::new().bg(t.bar));
    let name = project_name(path, cx.home, cx.scratch);
    let short = short_path(path, cx.home);
    let needs = members.iter().filter(|s| s.state == State::NeedsYou).count();
    let badge = if needs > 0 { format!("◆{needs}") } else { String::new() };
    let label_end = g.x0 + g.label_w;
    let x = put_str(buf, g.x0 + 1, y, "▾ ", fg(t.dim), 2);
    let room = label_end.saturating_sub(x + width(&badge) as u16 + 1);
    let x = put_str(buf, x, y, &trunc(&name, usize::from(room)), t.bold(t.bright), room);
    let room = label_end.saturating_sub(x + 2 + width(&badge) as u16 + 1);
    if short != name && room > 3 {
        put_str(buf, x + 2, y, &trunc(&short, usize::from(room)), fg(t.dim), room);
    }
    if needs > 0 {
        put_str(buf, label_end.saturating_sub(width(&badge) as u16), y, &badge, t.bold(t.needs), 4);
    }
    // The project's activity as one hairline, so a project reads at a glance.
    let mut any = vec![false; usize::from(g.tw)];
    for s in members {
        for (c, f) in coverage(g, s, cx.now).into_iter().enumerate() {
            any[c] |= f > 0.0;
        }
    }
    let hair = t.mix(t.border, t.dim, 0.3);
    for c in 0..g.tw {
        if any[usize::from(c)] {
            set(buf, g.tx + c, y, "▔", hair);
        } else if g.tick[usize::from(c)] {
            set(buf, g.tx + c, y, "┊", t.sep);
        }
    }
    set(buf, g.now_x, y, "│", t.fg);
    if g.gw > 0 {
        let n = members.len();
        put_str(buf, g.gx, y, &format!("{n} session{}", if n == 1 { "" } else { "s" }), fg(t.dim), g.gw);
    }
}

fn draw_session(buf: &mut Buffer, cx: &Ctx, g: &Geo, y: u16, s: &Session, selected: bool, odd: bool) {
    let t = cx.theme;
    let needs = s.state == State::NeedsYou;
    let base = match (needs, selected) {
        (true, true) => t.needs_sel,
        (true, false) => t.needs_bg,
        (false, true) => t.sel,
        (false, false) if odd => t.strip,
        (false, false) => t.bg,
    };
    fill(buf, Rect::new(g.x0, y, g.right - g.x0, 1), Style::new().bg(base));
    let sc = t.state(s.state);

    // Label column: selection edge, state glyph, title (and the age when compact).
    if selected {
        put_str(buf, g.x0, y, "▌", fg(sc), 1);
    }
    let icon = if s.state == State::Running { spinner(cx.tick) } else { glyph(s.state) };
    put_str(buf, g.x0 + 2, y, icon, if needs { t.bold(sc) } else { fg(sc) }, 1);
    let title_style = match s.state {
        State::NeedsYou => t.bold(t.needs),
        _ if selected => t.bold(t.bright),
        State::Running | State::Elsewhere => fg(t.bright),
        State::Ended => fg(t.dim),
        State::Stale => fg(t.mix(t.fg, t.dim, 0.5)),
        State::Idle => fg(t.fg),
    };
    let age = ago(s.last, cx.now);
    let age_w = if g.compact { width(&age) as u16 + 1 } else { 0 };
    let tw = g.label_w.saturating_sub(4 + age_w);
    put_str(buf, g.x0 + 4, y, &trunc(&s.title, usize::from(tw)), title_style, tw);
    if g.compact {
        let ax = (g.x0 + g.label_w).saturating_sub(width(&age) as u16);
        put_str(buf, ax, y, &age, fg(if needs { t.needs } else { t.dim }), age_w);
    }

    // Track: night shading and gridlines, then bars, then the wait.
    for c in 0..g.tw {
        let x = g.tx + c;
        set_bg(buf, x, y, shade(t, base, g.night[usize::from(c)]));
        if g.tick[usize::from(c)] {
            set(buf, x, y, "┊", t.sep);
        }
    }
    let bc = bar_color(t, s.state);
    let mut drew = false;
    let cov = coverage(g, s, cx.now);
    for (c, f) in cov.iter().enumerate() {
        if *f > 0.0 {
            set(buf, g.tx + c as u16, y, bar_glyph(*f), bc);
            drew = true;
        }
    }
    if matches!(s.state, State::Running | State::Elsewhere) {
        let tip = if s.state == State::Running { t.mix(t.running, t.bright, 0.45) } else { t.mix(t.elsewhere, t.bright, 0.4) };
        set(buf, g.tx + g.tw - 1, y, "▆", tip);
        drew = true;
    }
    if let Some(ws) = wait_start(s) {
        let wc = g.col(ws);
        let (dash, mark, mc) = if needs { ("╍", "◆", t.needs) } else { ("┄", "◇", t.stale) };
        let dash_c = if needs { t.needs } else { t.darken(t.stale, 0.8) };
        for c in (wc + 1).max(0)..i32::from(g.tw) {
            set(buf, g.tx + c as u16, y, dash, dash_c);
        }
        if g.on_track(wc) {
            drew = true;
            set(buf, g.tx + wc as u16, y, mark, mc);
            if let Some(cell) = buf.cell_mut((g.tx + wc as u16, y)) {
                cell.modifier.insert(Modifier::BOLD);
            }
        }
        if needs {
            let text = format!(" waiting {} ", duration(cx.now - ws).replace("<1m", "now"));
            let lw = width(&text) as i32;
            let wcc = wc.clamp(0, i32::from(g.tw) - 1);
            let lx = pill_x(&cov, wcc, lw, i32::from(g.tw));
            let pill_style = Style::new().fg(t.needs_ink).bg(t.needs).add_modifier(Modifier::BOLD);
            put_str(buf, g.tx + lx as u16, y, &text, pill_style, (lw as u16).min(g.tw));
        }
    }
    if !drew {
        let last = latest(s);
        if g.col(last) < 0 {
            let text = format!("◂ {} ago", ago(last, cx.now));
            let w = (width(&text) as u16).min(g.tw);
            let bg = bg_at(buf, g.tx, y);
            put_str(buf, g.tx, y, &text, Style::new().fg(t.faint).bg(bg).add_modifier(Modifier::ITALIC), w);
        }
    }
    set(buf, g.now_x, y, "│", t.fg);

    // State column.
    if g.gw > 0 {
        let word = match s.state {
            State::Elsewhere => "other terminal",
            _ => widgets::state_word(s),
        };
        let text = match s.state {
            State::Running | State::Elsewhere => word.to_string(),
            _ => format!("{word} · {}", ago(s.last, cx.now)),
        };
        let style = if needs { t.bold(t.needs) } else { fg(if s.state == State::Ended { t.dim } else { sc }) };
        put_str(buf, g.gx, y, &trunc(&text, usize::from(g.gw)), style, g.gw);
    }
}

// --- the detail panel ----------------------------------------------------------------------------

/// A labelled value, wrapped to at most `max` lines; returns the rows used.
#[allow(clippy::too_many_arguments)]
fn kv(buf: &mut Buffer, t: &Theme, x: u16, y: u16, w: u16, key: &str, val: &str, style: Style, bg: Color, max: usize) -> u16 {
    const KW: u16 = 10;
    if max == 0 {
        return 0;
    }
    put_str(buf, x, y, key, fg(t.dim).bg(bg), KW);
    let vw = w.saturating_sub(KW);
    let lines = wrap(val, usize::from(vw));
    let n = lines.len().min(max);
    for (k, l) in lines.iter().take(n).enumerate() {
        let l = if k == n - 1 && lines.len() > n { trunc(&format!("{l} …"), usize::from(vw)) } else { trunc(l, usize::from(vw)) };
        put_str(buf, x + KW, y + k as u16, &l, style.bg(bg), vw);
    }
    n.max(1) as u16
}

fn draw_detail(buf: &mut Buffer, r: Rect, cx: &Ctx, s: &Session) {
    let t = cx.theme;
    let needs = s.state == State::NeedsYou;
    let sc = t.state(s.state);
    let pbg = t.strip;
    let border = if needs { t.needs } else { t.darken(sc, 0.75) };
    let title = Line::from(vec![
        Span::styled(format!(" {} ", glyph(s.state)), t.bold(sc).bg(pbg)),
        Span::styled(trunc(&s.title, usize::from(r.width.saturating_sub(34))), t.bold(t.bright).bg(pbg)),
        Span::styled(" ", Style::new().bg(pbg)),
    ]);
    let inner = panel(buf, r, border, pbg, title);
    let right = format!(" {} · {} ", widgets::state_word(s), ago(s.last, cx.now));
    let rw = width(&right) as u16;
    if r.width > rw + 30 {
        put_str(buf, r.right() - rw - 2, r.y, &right, fg(sc).bg(pbg), rw);
    }
    let inner = Rect::new(inner.x + 1, inner.y, inner.width.saturating_sub(2), inner.height);
    if inner.height == 0 || inner.width < 12 {
        return;
    }
    let compact = r.width < COMPACT_BELOW;
    let (lw, rx, rw) = if compact {
        (inner.width, 0, 0)
    } else {
        let rw = 48u16.min(inner.width / 2);
        (inner.width - rw - 3, inner.right() - rw, rw)
    };
    let x = inner.x;
    let mut y = inner.y;
    let left = |y: u16| usize::from(inner.bottom().saturating_sub(y));

    // What it's waiting on: the loudest thing in the panel.
    if let Some(w) = &s.waiting {
        let since = wait_start(s).unwrap_or(s.last);
        let (head, c, tint) = if needs { ("WAITING ON YOU", t.needs, t.needs_bg) } else { ("WAS WAITING ON", t.stale, t.stale_bg) };
        let clock = local(since).map(|d| d.format("since %H:%M").to_string()).unwrap_or_default();
        let lines = wrap(w, usize::from(lw.saturating_sub(4)));
        let h = (1 + lines.len().min(2) as u16).min(inner.height);
        fill(buf, Rect::new(x, y, lw, h), Style::new().bg(tint));
        for k in 0..h {
            put_str(buf, x, y + k, "▌", fg(c).bg(tint), 1);
        }
        let hx = put_str(buf, x + 2, y, head, t.bold(c).bg(tint), lw.saturating_sub(2));
        let note = format!("{} · {clock}", duration(cx.now - since).replace("<1m", "now"));
        put_str(buf, hx + 2, y, &note, fg(t.darken(c, 0.8)).bg(tint), (x + lw).saturating_sub(hx + 2));
        for (k, l) in lines.iter().take(usize::from(h.saturating_sub(1))).enumerate() {
            let l = if k == 1 && lines.len() > 2 { trunc(&format!("{l} …"), usize::from(lw.saturating_sub(4))) } else { l.clone() };
            put_str(buf, x + 2, y + 1 + k as u16, &l, t.bold(t.bright).bg(tint), lw.saturating_sub(4));
        }
        y += h + 1;
    }
    let text = fg(t.fg);
    if let Some(a) = &s.asked {
        y += kv(buf, t, x, y, lw, "asked", a, text, pbg, 2.min(left(y)));
    }
    let status = s.summary.as_deref().or(s.said.as_deref()).unwrap_or("—");
    y += kv(buf, t, x, y, lw, "status", status, fg(t.bright), pbg, 2.min(left(y)));
    if let (Some(_), Some(said)) = (&s.summary, &s.said)
        && left(y) > 2
    {
        y += kv(buf, t, x, y, lw, "replied", said, text, pbg, 2.min(left(y) - 2));
    }
    let job = s.job.as_deref().map(|j| format!("job {j}")).unwrap_or_else(|| "interactive".into());
    let wher = format!("{}   {job}", short_path(&s.cwd, cx.home));
    y += kv(buf, t, x, y, lw, "where", &wher, fg(t.dim), pbg, 1.min(left(y)));

    // Context, numbers and the lifeline: a right-hand column, or stacked below when compact.
    let first = s.spans.first().map(|p| p.start).unwrap_or(s.last).min(s.last);
    let sp = spans(s, cx.now);
    let active: f64 = sp.iter().map(|(a, b)| b - a).sum();
    let bursts = sp.len();
    let model = s.model.as_deref().map(model_name).unwrap_or_else(|| "—".into());
    let (gx, mut gy, gw) = if compact { (x, y, lw) } else { (rx, inner.y, rw) };
    if left(gy) > 0 && gw > 14 {
        put_str(buf, gx, gy, "context", fg(t.dim).bg(pbg), 10);
        context_gauge(buf, t, gx + 10, gy, gw - 10, s.context, pbg);
        gy += 1;
    }
    let activity = format!("{bursts} burst{} · {} active", if bursts == 1 { "" } else { "s" }, duration(active));
    if compact {
        if left(gy) > 0 {
            let line = format!("{model}  ·  {} out  ·  {activity}", tokens(s.output));
            put_str(buf, gx, gy, &trunc(&line, usize::from(gw)), fg(t.dim).bg(pbg), gw);
            gy += 1;
        }
    } else {
        let last = match ago(s.last, cx.now).as_str() {
            "now" => "now".to_string(),
            a => format!("{a} ago"),
        };
        let started = format!("{} ago · last {last}", duration(cx.now - first));
        for (k, v) in [("output", format!("{} tokens", tokens(s.output))), ("model", model), ("activity", activity), ("started", started)] {
            if left(gy) == 0 {
                break;
            }
            put_str(buf, gx, gy, k, fg(t.dim).bg(pbg), 10);
            put_str(buf, gx + 10, gy, &trunc(&v, usize::from(gw.saturating_sub(10))), fg(t.fg).bg(pbg), gw.saturating_sub(10));
            gy += 1;
        }
        gy += 1;
    }
    // The lifeline: this session over its own lifetime, start to now.
    if left(gy) > 0 && gw > 20 {
        put_str(buf, gx, gy, "lifeline", fg(t.dim).bg(pbg), 10);
        let lw2 = gw - 10;
        let span = (cx.now - first).max(1.0);
        let ws = wait_start(s);
        for c in 0..lw2 {
            let a = first + span * f64::from(c) / f64::from(lw2);
            let b = first + span * f64::from(c + 1) / f64::from(lw2);
            let on = sp.iter().any(|(p, q)| *p < b && *q > a);
            let (sym, color) = if on {
                ("━", bar_color(t, s.state))
            } else if ws.is_some_and(|w| w < b) {
                if needs { ("╍", t.needs) } else { ("┄", t.stale) }
            } else {
                ("─", t.sep)
            };
            set(buf, gx + 10 + c, gy, sym, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::claude::transcript::Span as Active;

    #[test]
    fn durations_read_as_people_say_them() {
        assert_eq!(duration(10.0), "<1m");
        assert_eq!(duration(42.0 * 60.0), "42m");
        assert_eq!(duration(3600.0), "1h");
        assert_eq!(duration(79.0 * 60.0), "1h 19m");
        assert_eq!(duration(2.0 * 86_400.0 + 3.0 * 3600.0), "2d 3h");
        assert_eq!(duration(3.0 * 86_400.0), "3d");
    }

    #[test]
    fn a_running_session_is_active_until_now_and_short_spans_count() {
        let mut s = Session::new("a", "t", "/p", "/p", State::Running, 900.0);
        s.spans = vec![Active { start: 100.0, end: 100.0 }, Active { start: 500.0, end: 900.0 }];
        assert_eq!(spans(&s, 2000.0), vec![(100.0, 160.0), (500.0, 2000.0)]);
        s.state = State::Idle;
        assert_eq!(spans(&s, 2000.0), vec![(100.0, 160.0), (500.0, 900.0)]);
    }

    #[test]
    fn only_needs_you_and_stale_waits_have_a_start() {
        let mut s = Session::new("a", "t", "/p", "/p", State::NeedsYou, 900.0);
        assert_eq!(wait_start(&s), Some(900.0), "falls back to the last activity");
        s.waiting_since = Some(800.0);
        assert_eq!(wait_start(&s), Some(800.0));
        s.state = State::Stale;
        assert_eq!(wait_start(&s), None, "stale but never needed you");
        s.was = Some(State::NeedsYou);
        assert_eq!(wait_start(&s), Some(800.0));
        s.state = State::Idle;
        assert_eq!(wait_start(&s), None);
    }
}
