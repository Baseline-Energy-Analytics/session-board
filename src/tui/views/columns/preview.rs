//! Columns' third column, the conversation: drawn whole into a buffer of the column's width
//! (as tall as it needs), which the view then shows a window of.

use super::ago_text;
use crate::actions;
use crate::facts::{Session, State};
use crate::tui::app::Ctx;
use crate::tui::theme::fg;
use crate::tui::widgets::{
    self, ago, bubble, caps, context_gauge, hline, keycap, model_name, panel, pill, put, short_path, spans_width, spinner,
    state_pill, tokens, trunc, width, wrap,
};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

/// Draw `s` into `buf` (`w` wide, from the top); returns the height used.
pub(super) fn render(buf: &mut Buffer, w: u16, cx: &Ctx, s: &Session) -> u16 {
    let t = cx.theme;
    let x = 2u16;
    let w = w.saturating_sub(4).max(8);
    let mut y = 1u16;

    // Title, two lines at most.
    let mut lines = wrap(&s.title, usize::from(w));
    if lines.len() > 2 {
        lines.truncate(2);
        lines[1] = trunc(&format!("{} …", lines[1]), usize::from(w));
    }
    for l in lines {
        put(buf, x, y, &[Span::styled(l, t.bold(t.bright))], w);
        y += 1;
    }

    // State, age, folder.
    let mut line = vec![
        state_pill(t, s, None),
        Span::styled(format!("  {}", ago_text(s, cx.now)), fg(t.dim)),
    ];
    if !s.folder().is_empty() {
        line.push(Span::styled("  ·  ", fg(t.faint)));
        line.push(Span::styled(short_path(s.folder(), cx.home), fg(t.dim)));
    }
    put(buf, x, y + 1, &widgets::fit(&line, usize::from(w)), w);
    y += 3;

    if cx.prefs.show_columns {
        y = strip(buf, cx, x, y, w, s);
    }

    // The conversation: your latest prompt, then Claude's latest reply, which answers it.
    if let Some(a) = s.last_asked.as_deref().or(s.asked.as_deref()).filter(|a| !a.trim().is_empty()) {
        let latest = if s.prompts > 1 { format!(" · latest of {}", s.prompts) } else { String::new() };
        let label = [Span::styled("you", t.bold(t.accent)), Span::styled(latest, fg(t.faint))];
        y = bubble(buf, t, x, y, w, a, t.you_bg, &label);
    }
    let model = s.model.as_deref().map(model_name).unwrap_or_else(|| "Claude".into());
    let label = [Span::styled("claude", t.bold(t.claude)), Span::styled(format!(" · {model}"), fg(t.faint))];
    let reply = s.said.clone().or_else(|| if s.state == State::Running { None } else { s.summary.clone() });
    if let Some(r) = reply.filter(|r| !r.trim().is_empty()) {
        y = bubble(buf, t, x, y, w, &r, t.claude_bg, &label);
        if s.said.is_some()
            && let Some(sm) = s.summary.as_deref().filter(|m| !m.trim().is_empty())
        {
            let room = usize::from(w.saturating_sub(10));
            put(
                buf,
                x + 1,
                y - 1,
                &[Span::styled("status  ", t.bold(t.faint)), Span::styled(trunc(sm, room), fg(t.dim).add_modifier(Modifier::ITALIC))],
                w,
            );
            y += 1;
        }
    }
    if s.state == State::Running {
        let text = s.summary.clone().filter(|m| !m.trim().is_empty()).unwrap_or_else(|| "Working…".into());
        y = bubble(buf, t, x, y, w, &text, t.claude_bg, &label);
        let working = [
            Span::styled(format!("{} ", spinner(cx.tick)), fg(t.running)),
            Span::styled("working on a turn", fg(t.running).add_modifier(Modifier::ITALIC)),
        ];
        put(buf, x + 1, y - 1, &working, w);
        y += 1;
    }

    // What it needs from you, or what Enter does.
    let key = |k: &str| Span::styled(k.to_string(), t.bold(t.bright));
    let dim = |s: &str| Span::styled(s.to_string(), fg(t.dim));
    let dot = || Span::styled("  ·  ", fg(t.faint));
    match s.state {
        State::NeedsYou => y = waiting(buf, cx, x, y, w, s),
        State::Stale => y = stale(buf, cx, x, y, w, s),
        State::Ended => {
            let enter = match actions::plan_enter(s) {
                Ok(_) => vec![key("↵"), dim(" resumes it in the background")],
                Err(_) => vec![dim("its folder is gone, so it can't be resumed")],
            };
            let mut spans = vec![Span::styled("○ ", fg(t.ended)), dim(&format!("ended {}", ago_text(s, cx.now))), dot()];
            spans.extend(enter);
            y = system_line(buf, cx, x, y, w, spans);
        }
        State::Elsewhere => {
            let spans = vec![
                Span::styled("● ", fg(t.elsewhere)),
                dim("open in another terminal"),
                dot(),
                dim("/bg there lets the board enter it"),
            ];
            y = system_line(buf, cx, x, y, w, spans);
        }
        State::Idle => {
            let (what, enter) = if s.stopped { ("stopped", " opens it again") } else { ("idle, waiting for a prompt", " jump in") };
            let spans = vec![Span::styled("■ ", fg(t.idle)), dim(what), dot(), key("↵"), dim(enter)];
            y = system_line(buf, cx, x, y, w, spans);
        }
        State::Running => {
            let spans = vec![key("↵"), dim(" watch it work"), dot(), key("s"), dim(" stop")];
            y = system_line(buf, cx, x, y, w, spans);
        }
    }

    // What it has done: the files it touched, and where its folder stands in git.
    y = super::work::files_card(buf, cx, x, y, w, s);
    super::work::git_card(buf, cx, x, y, w, s)
}

/// The metadata strip: model, output tokens, job and id, then the context gauge (tokens against
/// a 1M scale, labelled). A soft-edged band; returns the row after it.
fn strip(buf: &mut Buffer, cx: &Ctx, x: u16, y: u16, w: u16, s: &Session) -> u16 {
    let t = cx.theme;
    let label = |l: &str| Span::styled(format!("{l} "), t.bold(t.faint).bg(t.strip));
    let value = |v: String| Span::styled(v, fg(t.bright).bg(t.strip));
    let mut chips: Vec<Vec<Span>> = vec![
        vec![label("MODEL"), value(s.model.as_deref().map(model_name).unwrap_or_else(|| "—".into()))],
        vec![label("OUT"), value(tokens(s.output))],
    ];
    if let Some(j) = &s.job {
        chips.push(vec![label("JOB"), value(j.clone())]);
    }
    chips.push(vec![label("ID"), Span::styled(s.session_id.chars().take(8).collect::<String>(), fg(t.dim).bg(t.strip))]);
    let inner = w.saturating_sub(4);
    let mut lines: Vec<Vec<Span>> = vec![Vec::new()];
    for c in chips {
        let cw = spans_width(&c);
        let Some(cur) = lines.last_mut() else { break };
        let used = spans_width(cur);
        if used > 0 && used + 4 + cw > usize::from(inner) {
            lines.push(c);
        } else {
            if used > 0 {
                cur.push(Span::styled("    ", fg(t.faint).bg(t.strip)));
            }
            cur.extend(c);
        }
    }
    let n = lines.len() as u16 + 1; // + the context line
    caps(buf, x, y, y + 1 + n, w, t.strip, t.bg);
    widgets::fill(buf, Rect::new(x, y + 1, w, n), Style::new().bg(t.strip));
    for (k, l) in lines.iter().enumerate() {
        put(buf, x + 2, y + 1 + k as u16, l, inner);
    }
    let cy = y + n;
    let lx = put(buf, x + 2, cy, &[label("CONTEXT")], inner);
    let gw = (x + 2 + inner).saturating_sub(lx).min(48);
    context_gauge(buf, t, lx, cy, gw, s.context, t.strip);
    y + n + 3
}

/// The amber "Waiting on you" panel: a pending tool call split into its tool and command, or
/// the question; then what Enter, `s` and `x` do.
fn waiting(buf: &mut Buffer, cx: &Ctx, x: u16, y: u16, w: u16, s: &Session) -> u16 {
    let t = cx.theme;
    let bg = t.needs_bg;
    let text = s.waiting.clone().filter(|w| !w.trim().is_empty()).unwrap_or_else(|| "Waiting for you".into());
    let inner = usize::from(w.saturating_sub(6));
    let mut body: Vec<Vec<Span>> = vec![Vec::new()];
    // A pending tool call from the transcript, as tool and argument.
    let tool = s.pending_tool.as_ref().map(|p| (p.name.as_str(), p.arg.as_str()));
    if let Some((name, cmd)) = tool {
        body.push(vec![Span::styled("Approve this tool call?", t.bold(t.bright))]);
        body.push(Vec::new());
        let lead = width(name) + 6;
        for (k, l) in wrap(cmd, inner.saturating_sub(lead).max(8)).into_iter().enumerate() {
            let mut line = if k == 0 {
                vec![pill(name, t.needs_ink, t.code), Span::styled("  $ ", fg(t.faint))]
            } else {
                vec![Span::raw(" ".repeat(lead))]
            };
            line.push(Span::styled(l, fg(t.code)));
            body.push(line);
        }
    } else {
        for l in wrap(&text, inner.max(8)) {
            body.push(vec![Span::styled(l, t.bold(t.bright))]);
        }
    }
    body.push(Vec::new());
    let mut act = vec![pill("↵ Jump in & reply", t.needs_ink, t.needs)];
    if actions::plan_stop(s).is_ok() {
        act.extend([Span::raw("   "), keycap(t, "s"), Span::styled(" stop", fg(t.fg))]);
    }
    if actions::plan_delete(s, cx.days).is_ok() {
        act.extend([Span::raw("   "), keycap(t, "x"), Span::styled(" delete", fg(t.fg))]);
    }
    if spans_width(&act) > inner {
        act[0] = pill("↵ Jump in", t.needs_ink, t.needs);
    }
    body.push(act);
    body.push(Vec::new());

    let since = s.waiting_since.unwrap_or(s.last);
    let title = Line::from(vec![
        Span::raw(" "),
        pill("◆ WAITING ON YOU", t.needs_ink, t.needs),
        Span::styled(format!(" for {} ", ago(since, cx.now)), fg(t.needs).bg(bg)),
    ]);
    boxed(buf, x, y, w, t.needs, bg, title, &body)
}

/// The "not running" panel for a stale session.
fn stale(buf: &mut Buffer, cx: &Ctx, x: u16, y: u16, w: u16, s: &Session) -> u16 {
    let t = cx.theme;
    let inner = usize::from(w.saturating_sub(6)).max(8);
    let mut body: Vec<Vec<Span>> = vec![Vec::new()];
    if let Some(wt) = s.waiting.as_deref().filter(|w| !w.trim().is_empty()) {
        let was = s.was.map_or("busy", |w| w.as_str());
        body.push(vec![Span::styled(format!("Was {was}: "), fg(t.dim)), Span::styled(wt.to_string(), t.bold(t.bright))]);
    }
    for l in wrap("Claude Code has no process for it, so its state is out of date (after a reboot, say).", inner) {
        body.push(vec![Span::styled(l, fg(t.dim))]);
    }
    body.push(Vec::new());
    body.push(vec![pill("↵ Start it again", t.needs_ink, t.stale)]);
    body.push(Vec::new());
    let title = Line::from(vec![Span::raw(" "), pill("◇ NOT RUNNING", t.needs_ink, t.stale), Span::raw(" ")]);
    boxed(buf, x, y, w, t.stale, t.stale_bg, title, &body)
}

/// A rounded panel around `body` (one line each); returns the row after it, plus a gap.
#[allow(clippy::too_many_arguments)]
pub(super) fn boxed(buf: &mut Buffer, x: u16, y: u16, w: u16, border: ratatui::style::Color, bg: ratatui::style::Color, title: Line, body: &[Vec<Span>]) -> u16 {
    let h = body.len() as u16 + 2;
    let inner = panel(buf, Rect::new(x, y, w, h), border, bg, title);
    for (k, l) in body.iter().enumerate() {
        put(buf, inner.x + 2, inner.y + k as u16, l, inner.width.saturating_sub(3));
    }
    y + h + 1
}

/// A centred note between two hairlines, cut from the end to fit: `── ↵ watch it work · s stop ──`.
fn system_line(buf: &mut Buffer, cx: &Ctx, x: u16, y: u16, w: u16, mut spans: Vec<Span>) -> u16 {
    let t = cx.theme;
    while spans_width(&spans) + 8 > usize::from(w) && spans.len() > 2 {
        spans.pop();
        if spans.last().is_some_and(|s| s.content.trim() == "·") {
            spans.pop();
        }
    }
    let spans = widgets::fit(&spans, usize::from(w.saturating_sub(2)));
    let tw = spans_width(&spans) as u16 + 2;
    let side = w.saturating_sub(tw) / 2;
    hline(buf, x, y, side, "─", fg(t.sep));
    hline(buf, x + side + tw, y, w.saturating_sub(side + tw), "─", fg(t.sep));
    put(buf, x + side + 1, y, &spans, tw);
    y + 2
}
