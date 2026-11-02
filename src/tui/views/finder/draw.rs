//! Drawing Finder: header, search prompt, results grouped by state, the preview, the footer.
//! Laid out by hand on the buffer so the look holds at every width; below [`WIDE`] columns
//! the preview stacks under a one-line-per-session list.

use super::matching::{Field, Hit};
use crate::actions;
use crate::facts::{Session, State as St};
use crate::tui::app::{Ctx, ViewId};
use crate::tui::theme::{Theme, fg};
use crate::tui::widgets::{
    self, CONTEXT_SCALE, LineEdit, ago, bar, bubble, fill, fit, glyph, hline, keycap, model_name, panel, pill, put,
    put_right, put_str, spans_width, spinner, state_word, tokens, trunc, view_tabs, wrap,
};
use chrono::{DateTime, Local};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Widget};

/// At and above this width the preview sits beside the list; below, under it.
pub const WIDE: u16 = 110;

/// What a frame needs from the view.
pub struct State<'a> {
    pub query: &'a LineEdit,
    pub hits: &'a [Hit],
    /// The selected hit.
    pub sel: Option<usize>,
    /// The list's first line on screen last frame.
    pub scroll: usize,
}

/// What a frame tells the view back.
pub struct Out {
    pub scroll: usize,
    /// Results that fit in the list.
    pub page: usize,
}

pub fn draw(buf: &mut Buffer, area: Rect, cx: &Ctx, st: &State) -> Out {
    let t = cx.theme;
    fill(buf, area, t.text());
    let wide = area.width >= WIDE;
    header(buf, Rect::new(area.x, area.y, area.width, 1), cx, wide);
    let foot = Rect::new(area.x, area.bottom() - 1, area.width, 1);
    footer(buf, foot, cx, st, wide);

    let search_h = if area.height >= 14 { 3 } else { 1 };
    let inner_w = area.width.saturating_sub(2);
    search(buf, Rect::new(area.x + 1, area.y + 1, inner_w, search_h), cx, st);

    let top = area.y + 1 + search_h;
    let body = Rect::new(area.x + 1, top, inner_w, foot.y.saturating_sub(top));
    let mut out = Out { scroll: st.scroll, page: 1 };
    if body.height < 3 || body.width < 10 {
        return out;
    }
    let preview_area = if wide {
        let lw = body.width * 41 / 100;
        out = results(buf, Rect::new(body.x, body.y, lw, body.height), cx, st, true);
        Rect::new(body.x + lw + 1, body.y, body.width.saturating_sub(lw + 1), body.height)
    } else {
        let needed = (st.hits.len() + groups(st.hits, cx)) as u16 + 2;
        let mut lh = needed.min(body.height * 2 / 5).max(5).min(body.height);
        if body.height - lh < 6 {
            lh = body.height; // no room for a useful preview: the list alone
        }
        out = results(buf, Rect::new(body.x, body.y, body.width, lh), cx, st, false);
        Rect::new(body.x, body.y + lh, body.width, body.height - lh)
    };
    if preview_area.height >= 3 && preview_area.width >= 20 {
        preview(buf, preview_area, cx, st, wide);
    }
    out
}

// --- small pieces --------------------------------------------------------------------------------

fn session<'a>(cx: &Ctx<'a>, h: &Hit) -> &'a Session {
    &cx.snap.sessions[h.index]
}

/// `text` as spans, the characters at `hl` (char indices) lit.
fn lit(t: &Theme, text: &str, hl: &[usize], base: Style) -> Vec<Span<'static>> {
    let on = base.fg(t.accent).add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut run_lit = false;
    for (i, c) in text.chars().enumerate() {
        let l = hl.binary_search(&i).is_ok();
        if l != run_lit && !run.is_empty() {
            out.push(Span::styled(std::mem::take(&mut run), if run_lit { on } else { base }));
        }
        run_lit = l;
        run.push(c);
    }
    if !run.is_empty() {
        out.push(Span::styled(run, if run_lit { on } else { base }));
    }
    out
}

fn s(text: impl Into<String>, style: Style) -> Span<'static> {
    Span::styled(text.into(), style)
}

/// Newlines and runs of spaces as single spaces, for one-line details.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The time of `epoch`, local: `16:15` today, `Tue 16:15` this week, `Oct 28 16:15` before.
pub fn clock(epoch: f64, now: f64) -> String {
    let at = |e: f64| DateTime::from_timestamp(e as i64, 0).map(|d| d.with_timezone(&Local));
    let (Some(when), Some(today)) = (at(epoch), at(now)) else { return "—".into() };
    if when.date_naive() == today.date_naive() {
        when.format("%H:%M").to_string()
    } else if now - epoch < 6.0 * 86400.0 {
        when.format("%a %H:%M").to_string()
    } else {
        when.format("%b %-d %H:%M").to_string()
    }
}

/// "5m ago", or "now".
fn ago_text(epoch: f64, now: f64) -> String {
    let a = ago(epoch, now);
    if a == "now" { a } else { format!("{a} ago") }
}

/// How a state group is headed.
fn group_name(state: St) -> &'static str {
    match state {
        St::NeedsYou => "NEEDS YOU",
        St::Running => "RUNNING",
        St::Elsewhere => "IN ANOTHER TERMINAL",
        St::Idle => "IDLE",
        St::Stale => "NOT RUNNING",
        St::Ended => "ENDED",
    }
}

fn groups(hits: &[Hit], cx: &Ctx) -> usize {
    let mut n = 0;
    let mut last = None;
    for h in hits {
        let s = Some(session(cx, h).state);
        if s != last {
            n += 1;
            last = s;
        }
    }
    n
}

// --- header, search, footer ----------------------------------------------------------------------

fn header(buf: &mut Buffer, r: Rect, cx: &Ctx, wide: bool) {
    let t = cx.theme;
    fill(buf, r, Style::new().bg(t.bar));
    let mut x = put(buf, r.x, r.y, &[pill("session-board", t.needs_ink, t.accent)], r.width);
    x = put(buf, x + 2, r.y, &view_tabs(t, ViewId::Finder, t.bar), r.right().saturating_sub(x + 2));

    let count = |state: St| cx.snap.sessions.iter().filter(|s| s.state == state).count();
    let on_bar = |c: Color| fg(c).bg(t.bar);
    // Groups, most important first: needs-you, the counts, the clock. What doesn't fit goes,
    // the counts first (from the right), then the clock.
    let need = count(St::NeedsYou);
    let needs: Vec<Span> = if need > 0 {
        let text = if wide { format!("◆ {need} need{} you", if need == 1 { "s" } else { "" }) } else { format!("◆ {need}") };
        vec![pill(&text, t.needs_ink, t.needs), s("  ", on_bar(t.dim))]
    } else {
        Vec::new()
    };
    let mut counts: Vec<Vec<Span>> = Vec::new();
    for state in [St::Running, St::Elsewhere, St::Idle, St::Stale] {
        let n = count(state);
        if n == 0 {
            continue;
        }
        let mut g = vec![s(glyph(state), on_bar(t.state(state))), s(format!(" {n}"), on_bar(t.fg))];
        if wide {
            let word = match state {
                St::Elsewhere => "elsewhere",
                St::Stale => "not running",
                other => other.as_str(),
            };
            g.push(s(format!(" {word}"), on_bar(t.dim)));
        }
        g.push(s("   ", on_bar(t.dim)));
        counts.push(g);
    }
    let mut clock_g = vec![s(format!("{} ", clock(cx.now, cx.now)), on_bar(t.faint))];
    let room = usize::from(r.right().saturating_sub(x + 2));
    loop {
        let right: Vec<Span> =
            needs.iter().chain(counts.iter().flatten()).chain(clock_g.iter()).cloned().collect();
        if spans_width(&right) <= room {
            put_right(buf, r.x, r.y, r.width, &right);
            return;
        }
        if counts.pop().is_none() && clock_g.drain(..).next().is_none() {
            if spans_width(&needs) <= room {
                put_right(buf, r.x, r.y, r.width, &needs);
            }
            return;
        }
    }
}

fn search(buf: &mut Buffer, r: Rect, cx: &Ctx, st: &State) {
    let t = cx.theme;
    let searching = !st.query.text().trim().is_empty();
    let line = if r.height >= 3 {
        let title = Line::from(s(" search ", t.bold(t.accent).bg(t.raised)));
        let inner = panel(buf, r, t.accent, t.raised, title);
        let (word, c) = if searching || cx.prefs.show_ended { ("shown", t.fg) } else { ("hidden", t.faint) };
        let note = [s(" ended ", fg(t.dim).bg(t.raised)), s(format!("{word} "), fg(c).bg(t.raised))];
        put_right(buf, r.x + 2, r.y, r.width.saturating_sub(4), &note);
        Rect::new(inner.x + 1, inner.y, inner.width.saturating_sub(2), 1)
    } else {
        fill(buf, r, Style::new().bg(t.raised));
        Rect::new(r.x + 1, r.y, r.width.saturating_sub(2), 1)
    };
    let total = cx.snap.sessions.len();
    let count = [s(st.hits.len().to_string(), t.bold(t.fg).bg(t.raised)), s(format!("/{total}"), fg(t.dim).bg(t.raised))];
    let cw = spans_width(&count) as u16;
    let x = put_str(buf, line.x, line.y, "› ", t.bold(t.accent).bg(t.raised), line.width);
    let qw = line.right().saturating_sub(x + cw + 2);
    let style = t.bold(t.bright).bg(t.raised);
    let (cx_, _) = st.query.render(buf, x, line.y, qw, style, true);
    if st.query.is_empty() {
        let hint = " Search titles, projects and folders…";
        put_str(buf, cx_ + 1, line.y, hint, fg(t.faint).bg(t.raised).add_modifier(Modifier::ITALIC), qw.saturating_sub(1));
    }
    if line.width > cw + 12 {
        put_right(buf, line.x, line.y, line.width, &count);
    }
}

fn footer(buf: &mut Buffer, r: Rect, cx: &Ctx, st: &State, wide: bool) {
    let t = cx.theme;
    fill(buf, r, Style::new().bg(t.bar));
    let sel = st.sel.and_then(|i| st.hits.get(i)).map(|h| session(cx, h));
    let can = |ok: Option<bool>| ok.unwrap_or(false);
    let enter = can(sel.map(|s| actions::plan_enter(s).is_ok()));
    let stop = can(sel.map(|s| actions::plan_stop(s).is_ok()));
    let delete = can(sel.map(|s| actions::plan_delete(s, cx.days).is_ok()));
    let esc = if st.query.is_empty() { "back" } else { "clear" };
    let keys: Vec<(&str, &str, bool)> = if wide {
        vec![
            ("↵", "jump in", enter),
            ("^K", "actions", true),
            ("^S", "stop", stop),
            ("^X", "delete", delete),
            ("^E", "ended", true),
            ("↑↓", "move", true),
            ("Tab", "view", true),
            ("Esc", esc, true),
            ("F1", "help", true),
        ]
    } else {
        vec![("↵", "jump in", enter), ("^K", "actions", true), ("↑↓", "move", true), ("Esc", esc, true), ("Tab", "view", true)]
    };
    let mut spans = Vec::new();
    for (k, label, on) in keys {
        spans.push(if on { keycap(t, k) } else { s(format!(" {k} "), fg(t.faint).bg(t.bar)) });
        spans.push(s(format!(" {label}   "), fg(if on { t.dim } else { t.faint }).bg(t.bar)));
    }
    put(buf, r.x + 1, r.y, &fit(&spans, usize::from(r.width.saturating_sub(2))), r.width.saturating_sub(2));
}

// --- results -------------------------------------------------------------------------------------

enum Row {
    Header(St, usize),
    Item(usize),
}

fn results(buf: &mut Buffer, r: Rect, cx: &Ctx, st: &State, wide: bool) -> Out {
    let t = cx.theme;
    let inner = panel(buf, r, t.border, t.bg, Line::from(s(" results ", fg(t.dim))));
    let mut out = Out { scroll: 0, page: 1 };
    if inner.height == 0 || inner.width < 4 {
        return out;
    }
    if st.hits.is_empty() {
        let (a, b) = if !cx.loaded {
            ("Reading sessions…".to_string(), String::new())
        } else if !st.query.is_empty() {
            (format!("No matches for “{}”", st.query.text()), "Esc clears the search".to_string())
        } else if cx.snap.sessions.is_empty() {
            ("No sessions yet".to_string(), String::new())
        } else {
            ("Nothing to show".to_string(), "Ctrl+E shows ended sessions; typing searches them".to_string())
        };
        put_str(buf, inner.x + 2, inner.y + 1, &a, fg(t.dim), inner.width.saturating_sub(3));
        put_str(buf, inner.x + 2, inner.y + 2, &b, fg(t.faint), inner.width.saturating_sub(3));
        return out;
    }

    let ih: usize = if wide { 2 } else { 1 };
    let mut rows = Vec::new();
    let mut last = None;
    for (i, h) in st.hits.iter().enumerate() {
        let state = session(cx, h).state;
        if Some(state) != last {
            let n = st.hits.iter().filter(|x| session(cx, x).state == state).count();
            rows.push(Row::Header(state, n));
            last = Some(state);
        }
        rows.push(Row::Item(i));
    }
    let height = |row: &Row| if let Row::Header(..) = row { 1 } else { ih };
    let total: usize = rows.iter().map(height).sum();
    let h = usize::from(inner.height);

    // Scroll only as far as needed to show the selection (with its group's header when it's
    // the first of its group).
    let mut scroll = st.scroll.min(total.saturating_sub(h));
    let mut y = 0;
    for (k, row) in rows.iter().enumerate() {
        if let Row::Item(i) = row
            && Some(*i) == st.sel
        {
            let top = if k > 0 && matches!(rows[k - 1], Row::Header(..)) { y - 1 } else { y };
            if top < scroll {
                scroll = top;
            }
            if y + ih > scroll + h {
                scroll = y + ih - h;
            }
        }
        y += height(row);
    }
    out.scroll = scroll;
    out.page = (h / ih).max(1);

    let bar_room = if total > h { 1 } else { 0 };
    let w = inner.width.saturating_sub(bar_room);
    let mut y = 0;
    for row in &rows {
        let rh = height(row);
        let top = y;
        y += rh;
        if top < scroll || top + rh > scroll + h {
            continue;
        }
        let ry = inner.y + (top - scroll) as u16;
        match row {
            Row::Header(state, n) => {
                let c = t.state(*state);
                let head = [
                    s(format!(" {} ", group_name(*state)), t.bold(c)),
                    s(format!("{n} "), fg(if *state == St::NeedsYou { c } else { t.dim })),
                ];
                let hx = put(buf, inner.x, ry, &head, w);
                let rule = if *state == St::NeedsYou { t.mix(t.needs, t.bg, 0.6) } else { t.sep };
                hline(buf, hx, ry, (inner.x + w).saturating_sub(hx + 1), "─", fg(rule));
            }
            Row::Item(i) => {
                let hit = &st.hits[*i];
                item(buf, Rect::new(inner.x, ry, w, rh as u16), cx, hit, Some(*i) == st.sel, wide);
            }
        }
    }
    widgets::scrollbar(buf, t, inner, total, h, scroll, true);
    out
}

fn item(buf: &mut Buffer, r: Rect, cx: &Ctx, hit: &Hit, selected: bool, wide: bool) {
    let t = cx.theme;
    let sn = session(cx, hit);
    let need = sn.state == St::NeedsYou;
    let c = t.state(sn.state);
    let bg = match (selected, need) {
        (true, true) => t.needs_sel,
        (true, false) => t.sel,
        (false, true) => t.needs_bg,
        _ => t.bg,
    };
    let base = Style::new().bg(bg);
    fill(buf, r, base);
    for dy in 0..r.height {
        put_str(buf, r.x, r.y + dy, if selected { "▌" } else { " " }, base.fg(c), 1);
    }
    let title_style = match sn.state {
        St::NeedsYou => base.fg(t.bright).add_modifier(Modifier::BOLD),
        St::Ended => base.fg(t.dim),
        _ if selected => base.fg(t.bright),
        _ => base.fg(t.fg),
    };
    let g = if sn.state == St::Running { spinner(cx.tick) } else { glyph(sn.state) };
    let age = [s(ago(sn.last, cx.now), base.fg(if need { t.needs } else { t.dim })), s(" ", base)];
    let aw = spans_width(&age) as u16;
    put_right(buf, r.x, r.y, r.width, &age);

    let f = &hit.fields;
    let mut l1 = vec![s(" ", base), s(g, base.fg(c).add_modifier(Modifier::BOLD)), s(" ", base)];
    l1.extend(lit(t, &f.title, hit.hl(Field::Title), title_style));
    if !wide {
        l1.push(s("  ", base));
        l1.extend(lit(t, &f.name, hit.hl(Field::Name), base.fg(t.faint)));
    }
    let room = r.width.saturating_sub(aw + 2);
    put(buf, r.x + 1, r.y, &fit(&l1, usize::from(room)), room);
    if !wide || r.height < 2 {
        return;
    }
    let (detail, dstyle) = detail(t, sn, base);
    let mut l2 = vec![s("   ", base)];
    l2.extend(lit(t, &f.name, hit.hl(Field::Name), base.fg(t.dim)));
    if let Some(d) = detail {
        l2.push(s("  ", base));
        l2.push(s(d, dstyle));
    }
    let room = r.width.saturating_sub(2);
    put(buf, r.x + 1, r.y + 1, &fit(&l2, usize::from(room)), room);
}

/// A result's second line: what it's waiting on, doing, or last said.
fn detail(t: &Theme, sn: &Session, base: Style) -> (Option<String>, Style) {
    let quiet = base.fg(t.faint).add_modifier(Modifier::ITALIC);
    let text = |o: &Option<String>| o.as_deref().map(one_line).filter(|x| !x.is_empty());
    match sn.state {
        St::NeedsYou => (text(&sn.waiting).map(|w| format!("→ {w}")), base.fg(t.needs)),
        St::Stale => (text(&sn.waiting).or_else(|| text(&sn.summary)), base.fg(t.mix(t.stale, t.bg, 0.2))),
        St::Running => (text(&sn.summary), base.fg(t.mix(t.running, t.bg, 0.25))),
        St::Elsewhere => (text(&sn.said).or_else(|| text(&sn.summary)), base.fg(t.dim)),
        St::Idle if sn.stopped => match text(&sn.summary) {
            Some(x) if x.to_lowercase().starts_with("stopped") => (Some(x), quiet),
            Some(x) => (Some(format!("Stopped · {x}")), quiet),
            None => (Some("Stopped".into()), quiet),
        },
        _ => (text(&sn.summary).or_else(|| text(&sn.said)), quiet),
    }
}

// --- preview -------------------------------------------------------------------------------------

fn preview(buf: &mut Buffer, r: Rect, cx: &Ctx, st: &State, wide: bool) {
    let t = cx.theme;
    let hit = st.sel.and_then(|i| st.hits.get(i));
    let inner = panel(buf, r, t.border, t.bg, Line::from(s(" preview ", fg(t.dim))));
    let Some(hit) = hit else {
        put_str(buf, inner.x + 2, inner.y + 1, "Nothing selected.", fg(t.dim), inner.width.saturating_sub(3));
        return;
    };
    let sn = session(cx, hit);
    let id = match &sn.job {
        Some(j) => format!(" job {j} "),
        None => format!(" {} ", sn.session_id.chars().take(8).collect::<String>()),
    };
    put_right(buf, r.x + 2, r.y, r.width.saturating_sub(4), &[s(id, fg(t.faint).bg(t.bg))]);

    // Draw into a buffer the size of the inside, so nothing spills past the panel.
    let pad = if wide { 2 } else { 1 };
    let area = Rect::new(inner.x + pad, inner.y, inner.width.saturating_sub(pad * 2), inner.height);
    if area.width < 10 || area.height == 0 {
        return;
    }
    let mut pb = Buffer::empty(area);
    fill(&mut pb, area, t.text());
    preview_body(&mut pb, area, cx, hit, sn, wide);
    buf.merge(&pb);
}

fn preview_body(buf: &mut Buffer, r: Rect, cx: &Ctx, hit: &Hit, sn: &Session, wide: bool) {
    let t = cx.theme;
    let (x, w, bottom) = (r.x, r.width, r.bottom());
    let mut y = r.y + u16::from(wide && r.height > 20);
    let f = &hit.fields;

    // Title and badge.
    let b = widgets::state_pill(t, sn, Some(cx.tick));
    let bw = spans_width(std::slice::from_ref(&b)) as u16;
    put_right(buf, x, y, w, &[b]);
    let title = lit(t, &f.title, hit.hl(Field::Title), t.bold(t.bright));
    let room = w.saturating_sub(bw + 2);
    put(buf, x, y, &fit(&title, usize::from(room)), room);
    y += 1;

    // Where, what, when.
    let dot = || s("  ·  ", fg(t.faint));
    let mut place = vec![s("▸ ", fg(t.accent))];
    place.extend(lit(t, &f.name, hit.hl(Field::Name), t.bold(t.fg)));
    if f.cwd != "~" || sn.project != cx.home {
        place.push(s("  ", fg(t.dim)));
        place.extend(lit(t, &f.cwd, hit.hl(Field::Cwd), fg(t.dim)));
    }
    let mut when = Vec::new();
    if let Some(m) = &sn.model {
        when.push(s(model_name(m), fg(t.fg)));
        when.push(dot());
    }
    when.push(s(format!("{} {}", state_word(sn), ago_text(sn.last, cx.now)), fg(t.dim)));
    if wide {
        place.push(dot());
        place.extend(when);
        put(buf, x, y, &fit(&place, usize::from(w)), w);
        y += 2;
    } else {
        put(buf, x, y, &fit(&place, usize::from(w)), w);
        let mut l = vec![s("  ", fg(t.dim))];
        l.extend(when);
        put(buf, x, y + 1, &fit(&l, usize::from(w)), w);
        y += 2;
    }

    // Context and output.
    if y + 2 < bottom {
        gauges(buf, x, y, w, cx, sn);
        y += 3;
    }

    // What it's waiting on.
    if matches!(sn.state, St::NeedsYou | St::Stale) && y + 3 < bottom {
        y = callout(buf, x, y, w, bottom, cx, sn) + 1;
    }

    // Details pinned to the bottom, when there's room for them and a conversation.
    let details_h = 5;
    let pinned = wide && bottom.saturating_sub(y) >= details_h + 8;
    let conv_bottom = if pinned { bottom - details_h - 1 } else { bottom };
    y = conversation(buf, x, y, w, conv_bottom, cx, sn);
    if pinned {
        details(buf, Rect::new(x, bottom - details_h, w, details_h), cx, hit, sn);
    } else if bottom.saturating_sub(y) > details_h {
        details(buf, Rect::new(x, y + 1, w, details_h), cx, hit, sn);
    }
}

fn gauges(buf: &mut Buffer, x: u16, y: u16, w: u16, cx: &Ctx, sn: &Session) {
    let t = cx.theme;
    let one = |buf: &mut Buffer, x: u16, gw: u16, label: &str, value: Vec<Span<'static>>| {
        put_str(buf, x, y, label, fg(t.dim), gw);
        put_right(buf, x, y, gw, &value);
    };
    let ctx_value = vec![s(tokens(sn.context), t.bold(t.bright)), s(" of 1M", fg(t.dim))];
    let ctx_frac = sn.context as f64 / CONTEXT_SCALE as f64;
    let out_value = vec![s(tokens(sn.output), t.bold(t.bright)), s(" tokens", fg(t.dim))];
    let busiest = cx.snap.sessions.iter().map(|s| s.output).max().unwrap_or(0).max(1);
    let out_frac = sn.output as f64 / busiest as f64;
    if w >= 50 {
        let gw = (w - 4) / 2;
        one(buf, x, gw, "context", ctx_value);
        bar(buf, x, y + 1, gw, ctx_frac, |p| t.gauge(p), t.track);
        let ox = x + gw + 4;
        let ow = w - gw - 4;
        one(buf, ox, ow, "output", out_value);
        bar(buf, ox, y + 1, ow, out_frac, |p| t.mix(t.elsewhere, t.accent, p), t.track);
    } else {
        let mut v = ctx_value;
        v.push(s(format!("  ·  out {}", tokens(sn.output)), fg(t.dim)));
        one(buf, x, w, "context", v);
        bar(buf, x, y + 1, w, ctx_frac, |p| t.gauge(p), t.track);
    }
}

/// The "waiting on you" (or "not running") box. Returns the row under it.
fn callout(buf: &mut Buffer, x: u16, y: u16, w: u16, bottom: u16, cx: &Ctx, sn: &Session) -> u16 {
    let t = cx.theme;
    let need = sn.state == St::NeedsYou;
    let (c, bg) = if need { (t.needs, t.needs_bg) } else { (t.stale, t.stale_bg) };
    let waiting = sn.waiting.as_deref().map(one_line).filter(|w| !w.is_empty());
    let body = match (&waiting, need) {
        (Some(w), _) => w.clone(),
        (None, true) => "Claude is waiting for your answer.".to_string(),
        (None, false) => sn
            .summary
            .as_deref()
            .map(one_line)
            .unwrap_or_else(|| "Its state says it's busy, but Claude Code has no process running it.".into()),
    };
    let tool = sn.pending_tool.as_ref().filter(|_| waiting.is_some()).map(|p| (p.name.as_str(), p.arg.as_str()));
    let text = tool.map_or(body.as_str(), |(_, rest)| rest);
    let iw = w.saturating_sub(4);
    let lead = if tool.is_some_and(|(n, _)| n == "Bash") { 2 } else { 0 };
    let mut lines = wrap(text, usize::from(iw.saturating_sub(lead)).max(1));
    // Borders, the tool row, a blank and the key hint.
    let fixed = 2 + u16::from(tool.is_some()) + 2;
    let room = bottom.saturating_sub(y).saturating_sub(fixed);
    if room == 0 {
        return y;
    }
    let max = usize::from(room).min(6);
    if lines.len() > max {
        lines.truncate(max);
        if let Some(l) = lines.last_mut() {
            *l = trunc(&format!("{l} …"), usize::from(iw));
        }
    }
    let h = lines.len() as u16 + fixed;
    let area = Rect::new(x, y, w, h);
    let mut title = if need {
        vec![pill("◆ WAITING ON YOU", t.needs_ink, t.needs)]
    } else {
        let was = sn.was.map(|w| format!(" · was {w}")).unwrap_or_default();
        vec![s(format!(" ◇ NOT RUNNING{was} "), t.bold(t.stale).bg(t.bg))]
    };
    if need && let Some(since) = sn.waiting_since {
        title.push(s(format!(" {} ", ago_text(since, cx.now)), fg(t.needs).bg(t.bg)));
    }
    Block::bordered()
        .border_type(if need { BorderType::Thick } else { BorderType::Rounded })
        .border_style(fg(c).bg(t.bg))
        .style(Style::new().bg(bg))
        .title(Line::from(title))
        .render(area, buf);
    let ix = x + 2;
    let mut yy = y + 1;
    if let Some((name, _)) = tool {
        let what = if name == "Bash" { "approve this command" } else { "approve this tool call" };
        let chip = s(format!(" {name} "), Style::new().fg(c).bg(t.mix(c, bg, 0.75)).add_modifier(Modifier::BOLD));
        put(buf, ix, yy, &[chip, s(format!("  {what}"), fg(t.dim).bg(bg))], iw);
        yy += 1;
    }
    for (i, l) in lines.iter().enumerate() {
        let mut spans = Vec::new();
        if lead > 0 {
            spans.push(s(if i == 0 { "$ " } else { "  " }, fg(t.faint).bg(bg)));
        }
        let code = lead > 0;
        spans.push(s(l.clone(), if code { t.bold(t.code).bg(bg) } else { t.bold(t.bright).bg(bg) }));
        put(buf, ix, yy, &spans, iw);
        yy += 1;
    }
    yy += 1;
    let hint = if need {
        vec![keycap(t, "↵"), s(" jump in to answer    ", fg(t.dim).bg(bg)), keycap(t, "^K"), s(" more actions", fg(t.dim).bg(bg))]
    } else {
        vec![keycap(t, "↵"), s(" jump in to pick it up", fg(t.dim).bg(bg))]
    };
    put(buf, ix, yy, &hint, iw);
    area.bottom()
}

/// `text` cut to `lines` lines of `width`, the last ending in `…` when cut; newline-joined so a
/// bubble of that width wraps it the same way.
fn clamp_lines(text: &str, width: usize, lines: usize) -> String {
    let mut all = wrap(text.trim(), width);
    if all.len() > lines {
        all.truncate(lines.max(1));
        if let Some(l) = all.last_mut() {
            *l = trunc(&format!("{l} …"), width);
        }
    }
    all.join("\n")
}

/// The chat: your ask, Claude's reply, and a line on where the session stands. Returns the row
/// under it.
fn conversation(buf: &mut Buffer, x: u16, y: u16, w: u16, bottom: u16, cx: &Ctx, sn: &Session) -> u16 {
    let t = cx.theme;
    let mut y = y;
    let note = note(t, sn, cx, w);
    let status = status(t, sn, cx.tick);
    // A running session's status is what it's doing now: it keeps its row.
    let reserved = note.len() as u16 + if sn.state == St::Running { 2 } else { 0 };
    if y + 2 < bottom {
        put_str(buf, x, y, "conversation ", fg(t.dim), w);
        hline(buf, x + 13, y, w.saturating_sub(13), "─", fg(t.sep));
        y += 2;
    }
    let width = (usize::from(w) * 72 / 100).max(12).saturating_sub(4);
    let asked = sn.asked.as_deref().map(str::trim).filter(|a| !a.is_empty());
    let said = sn.said.as_deref().map(str::trim).filter(|a| !a.is_empty());
    let reply = said.or(if sn.state == St::Running { None } else { sn.summary.as_deref() });
    let reply_is_summary = said.is_none() && reply.is_some();
    let avail = bottom.saturating_sub(y + reserved) as usize;
    let lines = |text: &str| wrap(text, width).len();
    // A bubble takes its lines plus three (label, two caps), and a gap after when there's room.
    // When both don't fit, Claude's reply wins: it's what you'd answer.
    let mut asked_n = 0;
    if let Some(a) = asked {
        asked_n = match reply {
            Some(_) if avail >= 10 => lines(a).min((avail - 10) / 3 + 1),
            Some(_) => 0,
            None if avail >= 4 => lines(a).min(avail - 3),
            None => 0,
        };
    }
    let after_asked = avail.saturating_sub(if asked_n > 0 { asked_n + 4 } else { 0 });
    let reply_n = match reply {
        Some(r) if after_asked >= 5 => lines(r).min(after_asked - 4),
        _ => 0,
    };

    let you = [s("you", t.bold(t.accent))];
    let claude = [s("✻ ", t.bold(t.claude)), s("claude", t.bold(t.claude))];
    if let Some(a) = asked.filter(|_| asked_n > 0) {
        y = bubble(buf, t, x, y, w, &clamp_lines(a, width, asked_n), true, t.you_bg, &you);
    }
    if let Some(r) = reply.filter(|_| reply_n > 0) {
        y = bubble(buf, t, x, y, w, &clamp_lines(r, width, reply_n), false, t.claude_bg, &claude);
        if reply_is_summary {
            y = y.saturating_sub(1);
        }
    }
    if let Some(line) = status
        && y + note.len() as u16 + u16::from(!note.is_empty()) <= bottom
    {
        put(buf, x, y, &fit(&line, usize::from(w)), w);
        y += 2;
    }
    for line in note {
        if y >= bottom {
            break;
        }
        put(buf, x, y, &fit(&line, usize::from(w)), w);
        y += 1;
    }
    y
}

/// What a session is doing, under the chat: a running one's status, or Claude Code's summary
/// of an idle one when the chat shows its last reply.
fn status(t: &Theme, sn: &Session, tick: u64) -> Option<Vec<Span<'static>>> {
    let summary = sn.summary.as_deref().map(one_line).filter(|x| !x.is_empty());
    if sn.state == St::Running {
        let text = summary.unwrap_or_else(|| "working…".into());
        return Some(vec![
            s("✻ ", t.bold(t.claude)),
            s(format!("{} ", spinner(tick)), fg(t.claude)),
            s(text, fg(t.running).add_modifier(Modifier::ITALIC)),
        ]);
    }
    let said = sn.said.as_deref().is_some_and(|x| !x.trim().is_empty());
    (said && summary.is_some()).then(|| {
        vec![s("  status · ", fg(t.faint)), s(summary.unwrap_or_default(), fg(t.dim).add_modifier(Modifier::ITALIC))]
    })
}

/// A line (or a few, wrapped to `w`) on what Enter would do, for the states without a callout.
fn note(t: &Theme, sn: &Session, cx: &Ctx, w: u16) -> Vec<Vec<Span<'static>>> {
    let key = || s("↵", t.bold(t.accent));
    let dim = |x: &str| s(x.to_string(), fg(t.dim));
    let wrapped = |mark: Span<'static>, text: &str, style: Style| -> Vec<Vec<Span<'static>>> {
        wrap(text, usize::from(w.saturating_sub(2)).max(1))
            .into_iter()
            .enumerate()
            .map(|(i, l)| vec![if i == 0 { mark.clone() } else { s("  ", style) }, s(l, style)])
            .collect()
    };
    vec![match sn.state {
        St::Ended => match actions::plan_enter(sn) {
            Ok(_) => vec![
                s("○ ", fg(t.ended)),
                dim(&format!("Ended {}", ago_text(sn.last, cx.now))),
                s("  —  ", fg(t.faint)),
                key(),
                dim(" resumes it in the background and jumps in"),
            ],
            Err(why) => return wrapped(s("○ ", fg(t.ended)), &why.0, fg(t.warn)),
        },
        St::Elsewhere => {
            let text = "Open in another terminal: switch to it there, or send it to the background with /bg \
                        to enter it from here.";
            return wrapped(s("● ", fg(t.elsewhere)), text, fg(t.dim));
        }
        St::Idle if sn.stopped => vec![s("■ ", fg(t.idle)), dim("Stopped.  "), key(), dim(" opens it again")],
        St::Idle => vec![s("■ ", fg(t.idle)), dim("Waiting for a prompt.  "), key(), dim(" to jump in")],
        _ => return Vec::new(),
    }]
}

fn details(buf: &mut Buffer, r: Rect, cx: &Ctx, hit: &Hit, sn: &Session) {
    let t = cx.theme;
    put_str(buf, r.x, r.y, "details ", fg(t.dim), r.width);
    hline(buf, r.x + 8, r.y, r.width.saturating_sub(8), "─", fg(t.sep));
    let f = &hit.fields;
    let job = match (&sn.job, sn.stopped) {
        (Some(j), true) => vec![s(j.clone(), fg(t.fg)), s("  stopped", fg(t.dim))],
        (Some(j), false) => vec![s(j.clone(), fg(t.fg))],
        (None, _) => vec![s("— not a background session", fg(t.faint))],
    };
    let model = vec![s(sn.model.clone().unwrap_or_else(|| "—".into()), fg(t.fg))];
    let last = vec![s(clock(sn.last, cx.now), fg(t.fg)), s(format!(" · {}", ago_text(sn.last, cx.now)), fg(t.dim))];
    let pairs: [(&str, Vec<Span>); 6] = [
        ("session", vec![s(sn.session_id.clone(), fg(t.fg))]),
        ("folder", lit(t, &f.cwd, hit.hl(Field::Cwd), fg(t.fg))),
        ("job", job),
        ("project", lit(t, &f.path, hit.hl(Field::Path), fg(t.fg))),
        ("last active", last),
        ("model", model),
    ];
    let cw = r.width.saturating_sub(2) / 2;
    for (i, (k, v)) in pairs.into_iter().enumerate() {
        let col = (i % 2) as u16;
        let row = (i / 2) as u16;
        let yy = r.y + 2 + row;
        if yy >= r.bottom() {
            break;
        }
        let xx = r.x + col * (cw + 2);
        let mut line = vec![s(format!("{k:<13}"), fg(t.faint))];
        line.extend(v);
        put(buf, xx, yy, &fit(&line, usize::from(cw)), cw);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lit_splits_runs() {
        let t = Theme::default();
        let spans = lit(&t, "infra", &[0, 1, 4], Style::new());
        let texts: Vec<&str> = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(texts, ["in", "fr", "a"]);
    }

    #[test]
    fn clamp_lines_cuts_with_an_ellipsis() {
        assert_eq!(clamp_lines("one two three four", 7, 1), "one tw…");
        assert_eq!(clamp_lines("one two", 7, 3), "one two");
    }
}
