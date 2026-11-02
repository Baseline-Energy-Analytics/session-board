//! Columns' first two columns: projects (with the pinned "Needs you" entry and a legend) and
//! the selected entry's sessions.

use super::{Board, Columns, Entry, sel_bar};
use crate::facts::{Session, State};
use crate::tui::app::Ctx;
use crate::tui::theme::fg;
use crate::tui::widgets::{self, ago, glyph, hline, pill, project_name, put, short_path, spans_width, state_word, trunc, width};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::Span;

/// A section rule with a label: `─ PROJECTS ─────`.
fn rule(buf: &mut Buffer, cx: &Ctx, x: u16, y: u16, w: u16, label: &str) {
    let t = cx.theme;
    hline(buf, x + 1, y, w.saturating_sub(2), "─", fg(t.sep));
    put(buf, x + 2, y, &[Span::styled(format!(" {label} "), t.bold(t.faint))], w.saturating_sub(3));
}

/// How the legend names a state.
fn legend_word(s: State) -> &'static str {
    match s {
        State::Elsewhere => "other terminal",
        State::Stale => "not running",
        s => s.as_str(),
    }
}

pub(super) fn projects(v: &mut Columns, buf: &mut Buffer, r: Rect, cx: &Ctx, b: &Board, focused: bool) {
    let t = cx.theme;
    if r.height < 4 || r.width < 8 {
        return;
    }
    let entry = v.entry();

    // The pinned "Needs you".
    let y = r.y + 1;
    let row = Rect::new(r.x, y, r.width, 1);
    if entry == Entry::NeedsYou {
        sel_bar(buf, t, row, focused, true);
    }
    let n = b.needs.len();
    put(
        buf,
        r.x,
        y,
        &[Span::styled("  ◆ ", t.bold(t.needs)), Span::styled("Needs you", t.bold(if n > 0 { t.bright } else { t.dim }))],
        r.width.saturating_sub(5),
    );
    let badge = if n > 0 { pill(&n.to_string(), t.needs_ink, t.needs) } else { Span::styled("✓", fg(t.running)) };
    widgets::put_right(buf, r.x, y, r.width.saturating_sub(1), &[badge]);

    // Projects, then the legend if there's room for both.
    rule(buf, cx, r.x, y + 2, r.width, "PROJECTS");
    let top = y + 3;
    let avail = usize::from(r.bottom().saturating_sub(top));
    let legend_h = State::ALL.len() + 2;
    let legend = avail > b.projects.len() + legend_h;
    let h = if legend { avail - legend_h } else { avail };
    if b.projects.is_empty() {
        let note = if cx.loaded { "none yet" } else { "reading…" };
        put(buf, r.x + 4, top, &[Span::styled(note, fg(t.faint))], r.width.saturating_sub(4));
    }

    // Keep the selected project in view.
    let sel = b.projects.iter().position(|(p, _)| Entry::Project(p.clone()) == entry);
    if let Some(i) = sel {
        if i < v.projects_top {
            v.projects_top = i;
        } else if h > 0 && i >= v.projects_top + h {
            v.projects_top = i + 1 - h;
        }
    }
    v.projects_top = v.projects_top.min(b.projects.len().saturating_sub(h));
    for (k, (path, ss)) in b.projects.iter().enumerate().skip(v.projects_top).take(h) {
        let y = top + (k - v.projects_top) as u16;
        let row = Rect::new(r.x, y, r.width, 1);
        let selected = sel == Some(k);
        if selected {
            sel_bar(buf, t, row, focused, false);
        }
        let needs = ss.iter().filter(|s| s.state == State::NeedsYou).count();
        let live = ss.iter().any(|s| matches!(s.state, State::Running | State::Elsewhere));
        let all_ended = ss.iter().all(|s| s.state == State::Ended);
        let right = if needs > 0 {
            pill(&needs.to_string(), t.needs_ink, t.needs)
        } else {
            Span::styled(format!("{} ", ss.len()), fg(t.faint))
        };
        let rw = spans_width(std::slice::from_ref(&right)) as u16;
        let name_color = if all_ended {
            t.ended
        } else if selected {
            t.bright
        } else {
            t.fg
        };
        let mut name = fg(name_color);
        if selected {
            name = name.add_modifier(Modifier::BOLD);
        }
        let room = r.width.saturating_sub(rw + 6);
        let spans = [
            Span::raw("  "),
            Span::styled(if live { "● " } else { "  " }, fg(t.running)),
            Span::styled(trunc(&project_name(path, cx.home, cx.scratch), usize::from(room)), name),
        ];
        put(buf, r.x, y, &spans, r.width.saturating_sub(rw + 1));
        widgets::put_right(buf, r.x, y, r.width.saturating_sub(1), &[right]);
    }
    if !legend {
        let bar = Rect::new(r.x, top, r.width, h as u16);
        widgets::scrollbar(buf, t, bar, b.projects.len(), h, v.projects_top, focused);
        return;
    }

    // Every session by state, ended ones counted even while hidden.
    let mut y = r.bottom() - legend_h as u16 + 1;
    rule(buf, cx, r.x, y, r.width, "ALL SESSIONS");
    y += 1;
    for s in State::ALL {
        let c = cx.snap.sessions.iter().filter(|x| x.state == s).count();
        let on = c > 0;
        let mut spans = vec![
            Span::styled(format!("  {} ", glyph(s)), fg(if on { t.state(s) } else { t.faint })),
            Span::styled(format!("{c:>2} "), t.bold(if on { t.fg } else { t.faint })),
            Span::styled(legend_word(s), fg(if on { t.dim } else { t.faint })),
        ];
        if s == State::Ended && on && !cx.prefs.show_ended {
            spans.push(Span::styled(" · hidden", fg(t.faint)));
        }
        put(buf, r.x, y, &spans, r.width);
        y += 1;
    }
}

/// A row of the sessions list.
enum Item<'a> {
    Row(&'a Session),
    /// The ENDED divider, with how many follow.
    Divider(usize),
}

impl Item<'_> {
    fn height(&self) -> u16 {
        match self {
            Item::Row(_) => 3,
            Item::Divider(_) => 2,
        }
    }
}

pub(super) fn sessions(v: &mut Columns, buf: &mut Buffer, r: Rect, cx: &Ctx, b: &Board, sel: Option<&Session>, focused: bool) {
    let t = cx.theme;
    if r.height < 3 || r.width < 8 {
        return;
    }
    let entry = v.entry();
    let list = b.list(&entry);
    let virt = entry == Entry::NeedsYou;

    // The column's label.
    let (label, extra) = match &entry {
        Entry::NeedsYou => ("NEEDS YOU".to_string(), if list.is_empty() { String::new() } else { format!("{} waiting, newest first", list.len()) }),
        Entry::Project(p) => (
            project_name(p, cx.home, cx.scratch).to_uppercase(),
            if p == cx.home { String::new() } else { short_path(p, cx.home) },
        ),
    };
    put(
        buf,
        r.x,
        r.y,
        &[Span::styled(format!(" {label}"), t.bold(t.dim)), Span::styled(format!("  {extra}"), fg(t.faint))],
        r.width,
    );

    if list.is_empty() {
        let (head, note) = if !cx.loaded {
            ("Reading sessions…", "")
        } else if virt {
            ("✓ Nothing needs you", "Every session is running, idle or done.")
        } else {
            ("No sessions here", "")
        };
        let color = if virt && cx.loaded { t.running } else { t.dim };
        put(buf, r.x + 3, r.y + 2, &[Span::styled(head, t.bold(color))], r.width.saturating_sub(4));
        for (k, l) in widgets::wrap(note, usize::from(r.width.saturating_sub(5))).iter().enumerate() {
            put(buf, r.x + 3, r.y + 3 + k as u16, &[Span::styled(l.clone(), fg(t.dim))], r.width.saturating_sub(4));
        }
        return;
    }

    let mut items = Vec::new();
    for (k, s) in list.iter().enumerate() {
        if !virt && s.state == State::Ended && !items.iter().any(|i| matches!(i, Item::Divider(_))) {
            items.push(Item::Divider(list.len() - k));
        }
        items.push(Item::Row(s));
    }

    // Lay out, then scroll as little as needed to keep the selection whole.
    let area = Rect::new(r.x, r.y + 2, r.width, r.height.saturating_sub(2));
    v.sessions_page = area.height;
    let mut ys = Vec::with_capacity(items.len());
    let mut total = 0u16;
    let mut sel_y = None;
    for it in &items {
        ys.push(total);
        if let (Item::Row(s), Some(cur)) = (it, sel)
            && s.session_id == cur.session_id
        {
            sel_y = Some(total);
        }
        total += it.height();
    }
    let total = total.saturating_sub(1); // no gap after the last row
    if let Some(sy) = sel_y {
        if sy < v.sessions_top {
            v.sessions_top = sy;
        } else if sy + 2 > v.sessions_top + area.height {
            v.sessions_top = (sy + 2).saturating_sub(area.height);
        }
    }
    v.sessions_top = v.sessions_top.min(total.saturating_sub(area.height));
    let room = area.width.saturating_sub(1); // the scrollbar's column
    for (it, &iy) in items.iter().zip(&ys) {
        let Some(rel) = iy.checked_sub(v.sessions_top) else { continue };
        if rel >= area.height {
            break;
        }
        let y = area.y + rel;
        match it {
            Item::Divider(n) => rule(buf, cx, area.x, y, room, &format!("ENDED · {n}")),
            Item::Row(s) => {
                let selected = sel.is_some_and(|c| c.session_id == s.session_id);
                let lines = (area.bottom() - y).min(2);
                if selected {
                    sel_bar(buf, t, Rect::new(area.x, y, room, lines), focused, s.state == State::NeedsYou);
                }
                row(buf, cx, Rect::new(area.x, y, room, lines), s, b, virt, selected);
            }
        }
    }
    widgets::scrollbar(buf, t, area, usize::from(total), usize::from(area.height), usize::from(v.sessions_top), focused);
}

/// One session, two lines: glyph, title and age; then its state (or, under "Needs you", its
/// project) and what it needs or is doing.
fn row(buf: &mut Buffer, cx: &Ctx, r: Rect, s: &Session, b: &Board, virt: bool, selected: bool) {
    let t = cx.theme;
    let st = s.state;
    let ended = st == State::Ended;
    let hot = st == State::NeedsYou;

    let age = ago(s.last, cx.now);
    let mut glyph_style = fg(t.state(st));
    if hot {
        glyph_style = glyph_style.add_modifier(Modifier::BOLD);
    }
    if st == State::Running && cx.tick % 4 >= 2 {
        glyph_style = fg(t.darken(t.running, 0.6));
    }
    let title_color = if ended {
        t.ended
    } else if selected || hot {
        t.bright
    } else {
        t.fg
    };
    let mut title_style = fg(title_color);
    if hot || selected {
        title_style = title_style.add_modifier(Modifier::BOLD);
    }
    let tw = usize::from(r.width).saturating_sub(width(&age) + 7);
    put(
        buf,
        r.x,
        r.y,
        &[Span::raw("  "), Span::styled(glyph(st), glyph_style), Span::raw(" "), Span::styled(trunc(&s.title, tw), title_style)],
        r.width,
    );
    let age_color = if hot {
        t.needs
    } else if ended {
        t.faint
    } else {
        t.dim
    };
    widgets::put_right(buf, r.x, r.y, r.width.saturating_sub(1), &[Span::styled(age, fg(age_color))]);
    if r.height < 2 {
        return;
    }

    let word = if virt {
        b.projects.iter().find(|(_, ss)| ss.iter().any(|x| x.session_id == s.session_id)).map_or(String::new(), |(p, _)| {
            project_name(p, cx.home, cx.scratch)
        })
    } else if r.width < 40 && !hot {
        String::new()
    } else {
        state_word(s).to_string()
    };
    let detail = match st {
        State::NeedsYou | State::Stale => s.waiting.clone(),
        State::Running | State::Idle => s.summary.clone(),
        _ => s.said.clone().or_else(|| s.summary.clone()),
    }
    .map(|d| d.split_whitespace().collect::<Vec<_>>().join(" "))
    // A stopped job's summary is just "Stopped.": the state word says it already.
    .filter(|d| !d.trim_end_matches('.').eq_ignore_ascii_case(state_word(s)))
    .unwrap_or_default();
    let word_color = if virt {
        t.accent
    } else if ended {
        t.faint
    } else {
        t.state(st)
    };
    let detail_color = if hot {
        t.mix(t.needs, t.bright, 0.55)
    } else if ended {
        t.faint
    } else {
        t.dim
    };
    let mut spans = vec![Span::raw("    "), Span::styled(word.clone(), fg(word_color))];
    if !detail.is_empty() {
        if !word.is_empty() {
            spans.push(Span::raw("  "));
        }
        let room = usize::from(r.width).saturating_sub(width(&word) + 8);
        spans.push(Span::styled(trunc(&detail, room), fg(detail_color)));
    }
    put(buf, r.x, r.y + 1, &spans, r.width.saturating_sub(1));
}
