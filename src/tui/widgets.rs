//! The shared drawing kit: one copy of each helper the three mockups each had their own of.
//! Everything is width-aware (`unicode-width`), clips to the buffer instead of panicking, and
//! takes its colours from a [`Theme`].
//!
//! Text helpers that `board --list` also uses (ages, paths, project names, state words, glyphs)
//! live in [`crate::report`] and are re-exported here, so a view needs one import.

use super::theme::{Theme, fg};
use crate::facts::State;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::{Offset, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Shadow, Widget};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub use crate::report::{ago, glyph, project_name, short_path, state_word};

// --- text ----------------------------------------------------------------------------------------

/// Display width in terminal cells.
pub fn width(s: &str) -> usize {
    s.width()
}

fn char_width(c: char) -> usize {
    c.width().unwrap_or(0)
}

/// `s` cut to `w` cells, ending in `…` when cut.
pub fn trunc(s: &str, w: usize) -> String {
    if s.width() <= w {
        return s.to_string();
    }
    if w == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = char_width(c);
        if used + cw + 1 > w {
            break;
        }
        out.push(c);
        used += cw;
    }
    out.push('…');
    out
}

/// `s` word-wrapped to `w` cells. Newlines start new lines (an empty line stays empty); a word
/// longer than a line (a path, a hash) is broken.
pub fn wrap(s: &str, w: usize) -> Vec<String> {
    let w = w.max(1);
    let mut lines = Vec::new();
    for para in s.split('\n') {
        let mut line = String::new();
        let mut lw = 0;
        for word in para.split_whitespace() {
            let ww = word.width();
            if lw > 0 && lw + 1 + ww <= w {
                line.push(' ');
                line.push_str(word);
                lw += 1 + ww;
                continue;
            }
            if lw > 0 {
                lines.push(std::mem::take(&mut line));
                lw = 0;
            }
            if ww <= w {
                line.push_str(word);
                lw = ww;
                continue;
            }
            for c in word.chars() {
                let cw = char_width(c);
                if lw + cw > w && lw > 0 {
                    lines.push(std::mem::take(&mut line));
                    lw = 0;
                }
                line.push(c);
                lw += cw;
            }
        }
        lines.push(line);
    }
    lines
}

/// Token counts as people read them: `950`, `182k` (rounded thousands), `1.2M`.
pub fn tokens(n: u64) -> String {
    if n < 1000 {
        n.to_string()
    } else if n < 1_000_000 {
        format!("{}k", (n as f64 / 1000.0).round_ties_even())
    } else {
        format!("{:.1}M", n as f64 / 1e6)
    }
}

/// `claude-opus-5-5` → `Opus 5.5`, `claude-sonnet-4-5-20250929` → `Sonnet 4.5`; anything else
/// cut to 10 characters.
pub fn model_name(model: &str) -> String {
    let parsed = (|| {
        let rest = model.strip_prefix("claude-")?;
        let (family, rest) = rest.split_once('-')?;
        if family.is_empty() || !family.chars().all(|c| c.is_ascii_lowercase()) {
            return None;
        }
        let digits = |s: &str| s.chars().take_while(char::is_ascii_digit).count();
        let n = digits(rest);
        if n == 0 {
            return None;
        }
        let (major, rest) = rest.split_at(n);
        let minor = rest.strip_prefix('-').and_then(|r| {
            let m = digits(r);
            (1..=2).contains(&m).then(|| &r[..m])
        });
        let mut name: String = family[..1].to_uppercase() + &family[1..];
        name.push(' ');
        name.push_str(major);
        if let Some(m) = minor {
            name.push('.');
            name.push_str(m);
        }
        Some(name)
    })();
    parsed.unwrap_or_else(|| model.chars().take(10).collect())
}

/// A running session's spinner frame for animation tick `tick` (one tick per 250 ms).
pub fn spinner(tick: u64) -> &'static str {
    const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    FRAMES[(tick % 10) as usize]
}

// --- spans ---------------------------------------------------------------------------------------

/// Total display width of `spans`.
pub fn spans_width(spans: &[Span]) -> usize {
    spans.iter().map(|s| s.content.width()).sum()
}

/// `spans` cut to `w` cells, the last kept span ending in `…` when cut.
pub fn fit<'a>(spans: &[Span<'a>], w: usize) -> Vec<Span<'a>> {
    if spans_width(spans) <= w {
        return spans.to_vec();
    }
    let mut out = Vec::new();
    let mut used = 0;
    for s in spans {
        let sw = s.content.width();
        if used + sw < w {
            out.push(s.clone());
            used += sw;
            continue;
        }
        out.push(Span::styled(trunc(&s.content, w - used), s.style));
        break;
    }
    out
}

/// Write `spans` at (x, y), clipped to `max` cells and to the buffer. Returns the x after the
/// last cell written.
pub fn put(buf: &mut Buffer, x: u16, y: u16, spans: &[Span], max: u16) -> u16 {
    let a = buf.area;
    if y < a.top() || y >= a.bottom() || x < a.left() || x >= a.right() {
        return x;
    }
    let end = x.saturating_add(max).min(a.right());
    let mut cx = x;
    for s in spans {
        if cx >= end {
            break;
        }
        cx = buf.set_stringn(cx, y, &s.content, usize::from(end - cx), s.style).0;
    }
    cx
}

/// [`put`] for one string.
pub fn put_str(buf: &mut Buffer, x: u16, y: u16, s: &str, style: Style, max: u16) -> u16 {
    put(buf, x, y, &[Span::styled(s, style)], max)
}

/// Write `spans` right-aligned within `[x, x + w)`, cut to fit. Returns where they start.
pub fn put_right(buf: &mut Buffer, x: u16, y: u16, w: u16, spans: &[Span]) -> u16 {
    let spans = fit(spans, usize::from(w));
    let sw = spans_width(&spans) as u16;
    let start = x + w.saturating_sub(sw);
    put(buf, start, y, &spans, sw);
    start
}

// --- areas ---------------------------------------------------------------------------------------

/// Blank `r` (clipped to the buffer) in `style`.
pub fn fill(buf: &mut Buffer, r: Rect, style: Style) {
    let r = r.intersection(buf.area);
    for y in r.top()..r.bottom() {
        for x in r.left()..r.right() {
            if let Some(c) = buf.cell_mut((x, y)) {
                c.reset();
                c.set_style(style);
            }
        }
    }
}

/// A row of `sym` from (x, y), `w` cells long.
pub fn hline(buf: &mut Buffer, x: u16, y: u16, w: u16, sym: &str, style: Style) {
    for i in 0..w {
        if let Some(c) = buf.cell_mut((x.saturating_add(i), y)) {
            c.set_symbol(sym).set_style(style);
        }
    }
}

/// A column of `sym` from (x, y), `h` cells tall.
pub fn vline(buf: &mut Buffer, x: u16, y: u16, h: u16, sym: &str, style: Style) {
    for i in 0..h {
        if let Some(c) = buf.cell_mut((x, y.saturating_add(i))) {
            c.set_symbol(sym).set_style(style);
        }
    }
}

/// Round the top and bottom of a band: a `▄` row at `top` and a `▀` row at `bottom`, in `bg`
/// over `outer`, so a filled block reads as a soft-edged card (Columns' bubbles and strip).
pub fn caps(buf: &mut Buffer, x: u16, top: u16, bottom: u16, w: u16, bg: Color, outer: Color) {
    hline(buf, x, top, w, "▄", Style::new().fg(bg).bg(outer));
    hline(buf, x, bottom, w, "▀", Style::new().fg(bg).bg(outer));
}

/// A rectangle of `w`x`h` centred across and a third of the way down `over`, kept inside it.
pub fn centered(over: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(over.width.saturating_sub(2)).max(1);
    let h = h.min(over.height.saturating_sub(2)).max(1);
    Rect::new(over.x + (over.width.saturating_sub(w)) / 2, over.y + (over.height.saturating_sub(h)) / 3, w, h)
}

// --- chips ---------------------------------------------------------------------------------------

/// Bold text on a coloured fill with a space either side: ` NEEDS YOU `.
pub fn pill(text: &str, fg: Color, bg: Color) -> Span<'static> {
    Span::styled(format!(" {text} "), Style::new().fg(fg).bg(bg).add_modifier(Modifier::BOLD))
}

/// A session's state pill in the board's words (NEEDS YOU, RUNNING, IN ANOTHER TERMINAL, IDLE
/// or STOPPED, NOT RUNNING, ENDED): amber fill for needs-you, a tinted fill otherwise. With
/// `tick`, a running session's glyph is the spinner.
pub fn state_pill(t: &Theme, s: &crate::facts::Session, tick: Option<u64>) -> Span<'static> {
    let mark = match (s.state, tick) {
        (State::Running, Some(k)) => spinner(k),
        _ => glyph(s.state),
    };
    let text = format!("{mark} {}", state_word(s).to_uppercase());
    match s.state {
        State::NeedsYou => pill(&text, t.needs_ink, t.needs),
        st => pill(&text, t.state(st), t.tint(st)),
    }
}

/// A key as a chip: ` ↵ `.
pub fn keycap(t: &Theme, key: &str) -> Span<'static> {
    Span::styled(format!(" {key} "), Style::new().fg(t.bright).bg(t.keycap).add_modifier(Modifier::BOLD))
}

/// A footer's key hints: keycap, label, gap, … on `bg`.
pub fn key_hints(t: &Theme, keys: &[(&str, &str)], bg: Color) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    for (k, label) in keys {
        out.push(keycap(t, k));
        out.push(Span::styled(format!(" {label}   "), Style::new().fg(t.dim).bg(bg)));
    }
    out
}

/// The three views as tabs, the active one lit: for a view's header.
pub fn view_tabs(t: &Theme, active: super::app::ViewId, bg: Color) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    for v in super::app::ViewId::ALL {
        let style = if v == active {
            Style::new().fg(t.bright).bg(t.sel).add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(t.dim).bg(bg)
        };
        out.push(Span::styled(format!(" {} ", v.title()), style));
    }
    out
}

// --- gauges --------------------------------------------------------------------------------------

/// A smooth bar in eighth blocks: `frac` of `w` cells filled, each cell coloured by
/// `color(position along the bar, 0..1)`, the rest `track`.
pub fn bar(buf: &mut Buffer, x: u16, y: u16, w: u16, frac: f64, color: impl Fn(f32) -> Color, track: Color) {
    const EIGHTHS: [&str; 8] = [" ", "▏", "▎", "▍", "▌", "▋", "▊", "▉"];
    let units = (frac.clamp(0.0, 1.0) * f64::from(w) * 8.0).round() as u32;
    for i in 0..w {
        let Some(cell) = buf.cell_mut((x.saturating_add(i), y)) else { break };
        let n = units.saturating_sub(u32::from(i) * 8).min(8);
        let pos = if w > 1 { f32::from(i) / f32::from(w - 1) } else { 0.0 };
        cell.set_symbol(if n == 8 { "█" } else { EIGHTHS[n as usize] }).set_fg(color(pos)).set_bg(track);
    }
}

/// The scale the context gauge is drawn against. Not the model's window, which the data
/// doesn't give (auto-compact has been seen anywhere from 285k to 580k), so it is labelled.
pub const CONTEXT_SCALE: u64 = 1_000_000;

/// The context gauge on one row: `182k ▉▉▉▏      of 1M`, `w` cells wide, on `bg`. The tokens
/// lead, the bar is green → amber → red, and the scale is spelled out.
pub fn context_gauge(buf: &mut Buffer, t: &Theme, x: u16, y: u16, w: u16, context: u64, bg: Color) {
    let value = format!("{} ", tokens(context));
    let scale = " of 1M";
    let vw = value.width() as u16;
    let sw = scale.width() as u16;
    let x2 = put_str(buf, x, y, &value, t.bold(t.bright).bg(bg), w);
    let bar_w = w.saturating_sub(vw + sw);
    if bar_w >= 4 {
        bar(buf, x2, y, bar_w, context as f64 / CONTEXT_SCALE as f64, |p| t.gauge(p), t.track);
        put_str(buf, x2 + bar_w, y, scale, fg(t.dim).bg(bg), sw);
    }
}

// --- containers ----------------------------------------------------------------------------------

/// A rounded, bordered panel filled with `bg`, titled; returns the inside.
pub fn panel(buf: &mut Buffer, area: Rect, border: Color, bg: Color, title: Line) -> Rect {
    let area = area.intersection(buf.area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(border).bg(bg))
        .style(Style::new().bg(bg))
        .title(title);
    let inner = block.inner(area);
    block.render(area, buf);
    inner
}

/// A popup `w`x`h` over `over`: cleared, rounded, titled top and bottom, with a shadow. Returns
/// the inside.
#[allow(clippy::too_many_arguments)]
pub fn popup(buf: &mut Buffer, t: &Theme, over: Rect, w: u16, h: u16, border: Color, title: Line, footer: Line) -> Rect {
    let area = centered(over, w, h);
    Clear.render(area, buf);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(border).bg(t.raised))
        .style(Style::new().fg(t.fg).bg(t.raised))
        .title(title)
        .title_bottom(footer.right_aligned())
        .shadow(Shadow::overlay().style(Style::new().bg(t.shadow).fg(t.faint)).offset(Offset::new(2, 1)));
    let inner = block.inner(area);
    block.render(area, buf); // the shadow is clipped to the buffer
    inner
}

/// A toast: a one-line message in a rounded box at the bottom right of `area`, `row` boxes up
/// from the bottom (0 = lowest). Nothing is drawn if it doesn't fit.
pub fn toast(buf: &mut Buffer, t: &Theme, area: Rect, row: u16, text: &str, color: Color) {
    let w = (text.width() as u16 + 6).min(area.width.saturating_sub(4));
    let h = 3;
    let bottom = area.bottom().saturating_sub(2 + row * h);
    if w < 10 || bottom < area.y + h {
        return;
    }
    let r = Rect::new(area.right().saturating_sub(w + 2), bottom - h, w, h);
    // Shadow, then the box.
    let shadow = Rect::new(r.x + 1, r.y + 1, w, h).intersection(area);
    for y in shadow.top()..shadow.bottom() {
        for x in shadow.left()..shadow.right() {
            if let Some(c) = buf.cell_mut((x, y)) {
                c.set_bg(t.shadow).set_fg(t.faint);
            }
        }
    }
    let inner = panel(buf, r, color, t.raised, Line::default());
    put_str(buf, inner.x + 1, inner.y, &trunc(text, usize::from(inner.width.saturating_sub(2))), fg(t.bright).bg(t.raised), inner.width);
}

/// A vertical scrollbar in the rightmost column of `r` for `content` rows seen through `view`
/// rows starting at `pos`. Nothing is drawn when everything fits.
pub fn scrollbar(buf: &mut Buffer, t: &Theme, r: Rect, content: usize, view: usize, pos: usize, focused: bool) {
    if content <= view || r.height == 0 || r.width == 0 {
        return;
    }
    let h = usize::from(r.height);
    let thumb = (h * view / content).clamp(1, h);
    let max_pos = content - view;
    let top = (h - thumb) * pos.min(max_pos) / max_pos.max(1);
    let x = r.right() - 1;
    for i in 0..h {
        let (sym, color) = if (top..top + thumb).contains(&i) {
            ("┃", if focused { t.accent } else { t.faint })
        } else {
            ("│", t.track)
        };
        if let Some(c) = buf.cell_mut((x, r.y + i as u16)) {
            c.set_symbol(sym).set_fg(color);
        }
    }
}

/// A chat bubble: a label line, then `text` wrapped in a soft-edged block of `bg`, at most 80% of
/// `avail` wide, on the left like everything else in a column. Light Markdown in the text
/// (`**bold**`, `` `code` ``, `# heading`) is drawn as style rather than shown as raw markers.
/// Returns the row after it (with a blank row of spacing).
#[allow(clippy::too_many_arguments)]
pub fn bubble(buf: &mut Buffer, t: &Theme, x: u16, y: u16, avail: u16, text: &str, bg: Color, label: &[Span]) -> u16 {
    let maxw = (usize::from(avail) * 80 / 100).max(12);
    let lines = md_wrap(text, maxw.saturating_sub(4));
    let line_w = |l: &[(String, Mark)]| l.iter().map(|(s, _)| s.width()).sum::<usize>();
    let inner = lines.iter().map(|l| line_w(l)).max().unwrap_or(0).max(spans_width(label));
    let bw = ((inner + 4) as u16).min(avail);
    put(buf, x + 1, y, label, avail);
    let n = lines.len() as u16;
    caps(buf, x, y + 1, y + 2 + n, bw, bg, t.bg);
    for (k, l) in lines.iter().enumerate() {
        let yy = y + 2 + k as u16;
        fill(buf, Rect::new(x, yy, bw, 1), Style::new().bg(bg));
        let spans: Vec<Span> = l
            .iter()
            .map(|(s, m)| {
                let style = match m {
                    Mark::Plain => fg(t.fg),
                    Mark::Bold => t.bold(t.bright),
                    Mark::Code => fg(t.code),
                };
                Span::styled(s.clone(), style.bg(bg))
            })
            .collect();
        put(buf, x + 2, yy, &spans, bw.saturating_sub(2));
    }
    y + 4 + n
}

/// How a run of bubble text is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Plain,
    Bold,
    Code,
}

/// Wrap `text` to `w` columns like [`wrap`], reading light Markdown on the way: `**bold**` and
/// `` `code` `` (the markers are dropped, and may span words) and `#` headings (bold). Anything
/// else, lists included, stays as written.
pub fn md_wrap(text: &str, w: usize) -> Vec<Vec<(String, Mark)>> {
    let w = w.max(1);
    let mut out: Vec<Vec<(String, Mark)>> = Vec::new();
    let (mut bold, mut code) = (false, false);
    for para in text.split('\n') {
        // A heading is bold throughout, without its #s.
        let trimmed = para.trim_start();
        let heading = trimmed.starts_with('#') && trimmed.trim_start_matches('#').starts_with(' ');
        let para = if heading { trimmed.trim_start_matches('#').trim_start() } else { para };

        // Words, each a list of styled pieces, with marker state carried across words.
        let mut words: Vec<Vec<(String, Mark)>> = Vec::new();
        for raw in para.split_whitespace() {
            let mut pieces: Vec<(String, Mark)> = Vec::new();
            let mut cur = String::new();
            let mark = |bold: bool, code: bool| if code { Mark::Code } else if bold || heading { Mark::Bold } else { Mark::Plain };
            let mut chars = raw.chars().peekable();
            while let Some(c) = chars.next() {
                let toggle = if c == '`' {
                    Some(false)
                } else if c == '*' && !code && chars.peek() == Some(&'*') {
                    chars.next();
                    Some(true)
                } else {
                    None
                };
                match toggle {
                    Some(is_bold) => {
                        if !cur.is_empty() {
                            pieces.push((std::mem::take(&mut cur), mark(bold, code)));
                        }
                        if is_bold { bold = !bold } else { code = !code }
                    }
                    None => cur.push(c),
                }
            }
            if !cur.is_empty() {
                pieces.push((cur, mark(bold, code)));
            }
            if !pieces.is_empty() {
                words.push(pieces);
            }
        }
        // Markers don't carry across paragraphs: an unclosed one ends with its line.
        bold = false;
        code = false;

        let mut line: Vec<(String, Mark)> = Vec::new();
        let mut lw = 0;
        for word in words {
            let ww: usize = word.iter().map(|(s, _)| s.width()).sum();
            if lw > 0 && lw + 1 + ww <= w {
                line.push((" ".into(), Mark::Plain));
                line.extend(word);
                lw += 1 + ww;
                continue;
            }
            if lw > 0 {
                out.push(std::mem::take(&mut line));
                lw = 0;
            }
            if ww <= w {
                line.extend(word);
                lw = ww;
                continue;
            }
            // Longer than a line: break it by characters.
            for (piece, m) in word {
                for c in piece.chars() {
                    let cw = char_width(c);
                    if lw + cw > w && lw > 0 {
                        out.push(std::mem::take(&mut line));
                        lw = 0;
                    }
                    match line.last_mut() {
                        Some((s, lm)) if *lm == m => s.push(c),
                        _ => line.push((c.to_string(), m)),
                    }
                    lw += cw;
                }
            }
        }
        out.push(line);
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    out
}

// --- text input ----------------------------------------------------------------------------------

/// What a key did to a [`LineEdit`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edit {
    /// Not an editing key; it's someone else's.
    Ignored,
    /// The cursor moved; the text is the same.
    Moved,
    /// The text changed.
    Changed,
}

/// A one-line text field: Finder's query and the new-session form's fields. Hand-rolled rather
/// than a crate, for exactly these keys: characters, Backspace, Delete, ←/→, Home/End, Ctrl+U
/// (delete to the start) and Ctrl+W (delete the word before the cursor).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LineEdit {
    text: String,
    /// Cursor position in chars, 0..=len.
    cursor: usize,
}

impl LineEdit {
    pub fn new(text: &str) -> LineEdit {
        LineEdit { text: text.to_string(), cursor: text.chars().count() }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// The cursor position in chars.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    pub fn set(&mut self, text: &str) {
        *self = LineEdit::new(text);
    }

    pub fn clear(&mut self) {
        *self = LineEdit::default();
    }

    fn byte(&self, char_pos: usize) -> usize {
        self.text.char_indices().nth(char_pos).map_or(self.text.len(), |(i, _)| i)
    }

    pub fn insert(&mut self, c: char) {
        let at = self.byte(self.cursor);
        self.text.insert(at, c);
        self.cursor += 1;
    }

    /// Delete chars `from..to` and leave the cursor at `from`.
    fn remove(&mut self, from: usize, to: usize) -> Edit {
        if from >= to {
            return Edit::Moved;
        }
        let (a, b) = (self.byte(from), self.byte(to));
        self.text.replace_range(a..b, "");
        self.cursor = from;
        Edit::Changed
    }

    /// Apply a key. Control characters other than the bindings above are ignored, so a
    /// caller can bind Ctrl+K and the like itself.
    pub fn handle(&mut self, key: &KeyEvent) -> Edit {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let len = self.text.chars().count();
        let moved = |me: &mut Self, to: usize| {
            me.cursor = to;
            Edit::Moved
        };
        match key.code {
            KeyCode::Char('u') if ctrl => self.remove(0, self.cursor),
            KeyCode::Char('w') if ctrl => {
                let chars: Vec<char> = self.text.chars().collect();
                let mut i = self.cursor;
                while i > 0 && chars[i - 1].is_whitespace() {
                    i -= 1;
                }
                while i > 0 && !chars[i - 1].is_whitespace() {
                    i -= 1;
                }
                self.remove(i, self.cursor)
            }
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                self.insert(c);
                Edit::Changed
            }
            KeyCode::Backspace if self.cursor > 0 => self.remove(self.cursor - 1, self.cursor),
            KeyCode::Backspace => Edit::Moved,
            KeyCode::Delete if self.cursor < len => self.remove(self.cursor, self.cursor + 1),
            KeyCode::Delete => Edit::Moved,
            KeyCode::Left => moved(self, self.cursor.saturating_sub(1)),
            KeyCode::Right => moved(self, (self.cursor + 1).min(len)),
            KeyCode::Home => moved(self, 0),
            KeyCode::End => moved(self, len),
            _ => Edit::Ignored,
        }
    }

    /// Draw the field at (x, y), `w` cells wide, scrolled so the cursor shows. With `cursor`,
    /// the cursor cell is drawn reversed (no terminal cursor to manage). Returns the cursor's
    /// screen position.
    pub fn render(&self, buf: &mut Buffer, x: u16, y: u16, w: u16, style: Style, cursor: bool) -> (u16, u16) {
        let w = usize::from(w);
        if w == 0 {
            return (x, y);
        }
        let chars: Vec<char> = self.text.chars().collect();
        let before: usize = chars[..self.cursor].iter().map(|&c| char_width(c)).sum();
        // First visible char: drop chars from the left until the cursor fits.
        let mut start = 0;
        let mut skipped = 0;
        while before - skipped >= w && start < self.cursor {
            skipped += char_width(chars[start]);
            start += 1;
        }
        let shown: String = chars[start..].iter().collect();
        put_str(buf, x, y, &shown, style, w as u16);
        let cx = x + (before - skipped) as u16;
        if cursor && let Some(c) = buf.cell_mut((cx, y)) {
            c.set_style(style.add_modifier(Modifier::REVERSED));
        }
        (cx, y)
    }
}

#[cfg(test)]
mod tests {
    fn md(text: &str, w: usize) -> Vec<Vec<(&'static str, Mark)>> {
        md_wrap(text, w)
            .into_iter()
            .map(|l| l.into_iter().map(|(s, m)| (&*Box::leak(s.into_boxed_str()), m)).collect())
            .collect()
    }

    #[test]
    fn md_wrap_draws_bold_and_code_without_markers() {
        let l = md("say **two words** and `ls -la` ok", 80);
        assert_eq!(l.len(), 1);
        let text: String = l[0].iter().map(|(s, _)| *s).collect();
        assert_eq!(text, "say two words and ls -la ok");
        assert!(l[0].contains(&("two", Mark::Bold)) && l[0].contains(&("words", Mark::Bold)));
        assert!(l[0].contains(&("ls", Mark::Code)) && l[0].contains(&("-la", Mark::Code)));
        assert!(l[0].contains(&("say", Mark::Plain)));
    }

    #[test]
    fn md_wrap_headings_wrapping_and_unclosed_markers() {
        let l = md("## Done\n**open bold\nnext line", 80);
        assert_eq!(l[0], vec![("Done", Mark::Bold)]);
        assert!(l[1].iter().all(|(_, m)| *m == Mark::Bold || *m == Mark::Plain));
        assert_eq!(l[2], vec![("next", Mark::Plain), (" ", Mark::Plain), ("line", Mark::Plain)]);
        let narrow = md("one two three four", 9);
        assert_eq!(narrow.len(), 3); // "one two" · "three" · "four"
        assert!(md("a * b ** c", 80)[0].iter().map(|(s, _)| *s).collect::<String>().contains("a * b"));
    }

    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn line_edit_keys() {
        let mut e = LineEdit::default();
        for c in "hello wörld".chars() {
            assert_eq!(e.handle(&key(KeyCode::Char(c))), Edit::Changed);
        }
        assert_eq!(e.handle(&ctrl('w')), Edit::Changed);
        assert_eq!(e.text(), "hello ");
        e.handle(&key(KeyCode::Home));
        e.handle(&key(KeyCode::Delete));
        e.handle(&key(KeyCode::Right));
        e.handle(&key(KeyCode::Char('X')));
        assert_eq!((e.text(), e.cursor()), ("eXllo ", 2));
        e.handle(&key(KeyCode::Backspace));
        e.handle(&key(KeyCode::Left));
        assert_eq!(e.handle(&ctrl('u')), Edit::Moved);
        assert_eq!((e.text(), e.cursor()), ("ello ", 0));
        e.handle(&key(KeyCode::End));
        assert_eq!(e.cursor(), 5);
        assert_eq!(e.handle(&ctrl('k')), Edit::Ignored);
        assert_eq!(e.handle(&key(KeyCode::Enter)), Edit::Ignored);
        e.handle(&ctrl('u'));
        assert!(e.is_empty());
    }

    #[test]
    fn text_helpers() {
        assert_eq!(trunc("abcdef", 4), "abc…");
        assert_eq!(trunc("ab", 4), "ab");
        assert_eq!(wrap("one two three", 7), ["one two", "three"]);
        assert_eq!(wrap("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("a\n\nb", 4), ["a", "", "b"]);
        assert_eq!(tokens(950), "950");
        assert_eq!(tokens(182_400), "182k");
        assert_eq!(tokens(2_500), "2k");
        assert_eq!(tokens(1_250_000), "1.2M");
        assert_eq!(model_name("claude-opus-5-5"), "Opus 5.5");
        assert_eq!(model_name("claude-sonnet-4-5-20250929"), "Sonnet 4.5");
        assert_eq!(model_name("claude-haiku-4-20250101"), "Haiku 4");
        assert_eq!(model_name("gpt-something-long"), "gpt-someth");
    }

    #[test]
    fn drawing_off_the_edge_is_clipped() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 3));
        let t = Theme::default();
        put_str(&mut buf, 8, 1, "hello", Style::new(), 20);
        assert_eq!(buf[(9, 1)].symbol(), "e");
        put_str(&mut buf, 20, 20, "x", Style::new(), 5);
        hline(&mut buf, 5, 2, 20, "─", Style::new());
        fill(&mut buf, Rect::new(5, 0, 50, 50), Style::new());
        bar(&mut buf, 6, 0, 10, 0.5, |_| t.running, t.track);
        toast(&mut buf, &t, Rect::new(0, 0, 10, 3), 0, "too big to fit", t.ok);
        scrollbar(&mut buf, &t, Rect::new(8, 0, 5, 9), 100, 3, 50, true);
        popup(&mut buf, &t, Rect::new(0, 0, 10, 3), 40, 20, t.accent, Line::default(), Line::default());
        context_gauge(&mut buf, &t, 2, 2, 30, 500_000, t.bg);
    }
}
