//! `board --shot`: one frame of the real app, drawn on ratatui's `TestBackend` and written as
//! an SVG. For reviewing the views without a terminal, and for tests.
//!
//! ```text
//! board --shot out.svg --size 160x45 [--view columns|timeline|finder] [--data snapshot.json]
//!       [--select <session id>] [--keys down,enter,…] [--home <dir>]
//! ```
//!
//! `--data` reads the `board --dump` shape (`tests/fixtures/sample.json`); without it the
//! sessions on this machine are collected. Keys are pressed in order, a frame drawn before each
//! as in the real loop; anything they would do outside the app (attach, refresh, running claude)
//! is skipped, so keys can open the help, a confirm, the new-session form or Finder's menu.
//! `--home` sets the folder shown as `~` (the fixture's `/home/u`, say) without touching `HOME`.
//!
//! The SVG draws backgrounds as rects, text on an exact grid, and block elements, box drawing
//! and braille as shapes, so bars and borders join without seams in any font.

use crate::actions::Cursor;
use crate::config::Config;
use crate::facts::{self, Snapshot, Sources};
use crate::tui::app::{App, ViewId};
use crate::tui::theme::{Depth, Theme};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Color, Modifier};
use std::fmt::Write;
use unicode_width::UnicodeWidthStr;

/// The parsed `--shot` options.
#[derive(Clone, Debug, PartialEq)]
pub struct Shot {
    pub out: String,
    pub width: u16,
    pub height: u16,
    pub view: ViewId,
    pub data: Option<String>,
    pub select: Option<String>,
    pub keys: Vec<KeyEvent>,
    /// The home folder for `~`; else the config's.
    pub home: Option<String>,
}

/// Read `--shot …` options; the error says what's wrong, for `board: …`.
pub fn parse(args: &[String]) -> Result<Shot, String> {
    let mut shot = Shot { out: String::new(), width: 160, height: 45, view: ViewId::Columns, data: None, select: None, keys: vec![], home: None };
    let mut size = None;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        let mut value = || it.next().cloned().ok_or_else(|| format!("{flag} needs a value"));
        match flag.as_str() {
            "--shot" => shot.out = value()?,
            "--size" => size = Some(value()?),
            "--view" => {
                let v = value()?;
                shot.view = ViewId::parse(&v).ok_or_else(|| format!("no view {v:?}; columns, timeline or finder"))?;
            }
            "--data" => shot.data = Some(value()?),
            "--select" => shot.select = Some(value()?),
            "--home" => shot.home = Some(value()?),
            "--keys" => {
                for name in value()?.split(',').filter(|k| !k.is_empty()) {
                    shot.keys.push(key(name).ok_or_else(|| format!("unknown key {name:?} in --keys"))?);
                }
            }
            other => return Err(format!("unknown option {other} with --shot")),
        }
    }
    let size = size.ok_or("--shot needs --size WxH")?;
    let (w, h) = size.split_once('x').ok_or_else(|| format!("--size {size}: expected WxH, like 160x45"))?;
    let (Ok(w), Ok(h)) = (w.parse::<u16>(), h.parse::<u16>()) else {
        return Err(format!("--size {size}: expected WxH, like 160x45"));
    };
    if w == 0 || h == 0 {
        return Err(format!("--size {size}: both sides must be at least 1"));
    }
    (shot.width, shot.height) = (w, h);
    Ok(shot)
}

/// A key by name: `a`, `?`, `enter`, `esc`, `tab`, `backtab`, `up`, `pgdn`, `f1`, `space`,
/// `comma`, `ctrl+k`, `alt+x`, `shift+tab`…
pub fn key(name: &str) -> Option<KeyEvent> {
    let mut mods = KeyModifiers::NONE;
    let mut rest = name;
    loop {
        let lower = rest.to_ascii_lowercase();
        if let Some(r) = lower.strip_prefix("ctrl+") {
            mods |= KeyModifiers::CONTROL;
            rest = &rest[rest.len() - r.len()..];
        } else if let Some(r) = lower.strip_prefix("alt+") {
            mods |= KeyModifiers::ALT;
            rest = &rest[rest.len() - r.len()..];
        } else if let Some(r) = lower.strip_prefix("shift+") {
            mods |= KeyModifiers::SHIFT;
            rest = &rest[rest.len() - r.len()..];
        } else {
            break;
        }
    }
    let mut chars = rest.chars();
    let code = match (chars.next(), chars.next()) {
        (Some(c), None) => KeyCode::Char(c),
        _ => match rest.to_ascii_lowercase().as_str() {
            "enter" | "return" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "tab" if mods.contains(KeyModifiers::SHIFT) => KeyCode::BackTab,
            "tab" => KeyCode::Tab,
            "backtab" => KeyCode::BackTab,
            "backspace" | "bs" => KeyCode::Backspace,
            "delete" | "del" => KeyCode::Delete,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pgup" | "pageup" => KeyCode::PageUp,
            "pgdn" | "pagedown" => KeyCode::PageDown,
            "space" => KeyCode::Char(' '),
            "comma" => KeyCode::Char(','),
            f if f.starts_with('f') => KeyCode::F(f[1..].parse().ok().filter(|n| (1..=12).contains(n))?),
            _ => return None,
        },
    };
    Some(KeyEvent::new(code, mods))
}

/// Make the shot: read or collect the snapshot, draw, write the SVG.
pub fn run(shot: &Shot, config: &Config) -> Result<(), String> {
    let snapshot = match &shot.data {
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|e| format!("--data {path}: {e}"))?;
            serde_json::from_str::<Snapshot>(&text).map_err(|e| format!("--data {path}: {e}"))?
        }
        None => facts::collect(&Sources::from_config(config), 1),
    };
    // True colour unless SESSION_BOARD_COLOR=256 asks to see the degrade.
    let depth = match std::env::var("SESSION_BOARD_COLOR").as_deref() {
        Ok("256") => Depth::Indexed256,
        _ => Depth::TrueColor,
    };
    let home = shot.home.as_deref().unwrap_or(&config.home);
    let mut app = App::offline(snapshot, home, &config.scratch, Theme::new(depth));
    app.settings.claude = config.claude_argv();
    app.view = shot.view;
    app.cursor = shot.select.clone().map(Cursor::Session);
    let buf = render(&mut app, shot.width, shot.height, &shot.keys);
    std::fs::write(&shot.out, svg(&buf)).map_err(|e| format!("{}: {e}", shot.out))
}

/// Press `keys` in `app` on a `width`x`height` `TestBackend`, a frame before each, and return
/// the last frame. Effects (attach, refresh) are dropped.
pub fn render(app: &mut App, width: u16, height: u16, keys: &[KeyEvent]) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("a TestBackend never fails");
    for k in keys {
        terminal.draw(|f| app.draw(f)).expect("a TestBackend never fails");
        app.handle_key(*k);
        app.take_effects();
    }
    terminal.draw(|f| app.draw(f)).expect("a TestBackend never fails");
    terminal.backend().buffer().clone()
}

// --- SVG -----------------------------------------------------------------------------------------

const CW: f64 = 9.0;
const CH: f64 = 19.0;
const FONT: f64 = 14.6;
const DEFAULT_FG: (u8, u8, u8) = (0xc9, 0xd1, 0xd9);
const DEFAULT_BG: (u8, u8, u8) = (0x0d, 0x11, 0x17);

/// The 16 terminal-themed colours, as a dark theme shows them.
const ANSI: [(u8, u8, u8); 16] = [
    (0x1b, 0x1f, 0x27), (0xe0, 0x6c, 0x75), (0x98, 0xc3, 0x79), (0xe5, 0xc0, 0x7b),
    (0x61, 0xaf, 0xef), (0xc6, 0x78, 0xdd), (0x56, 0xb6, 0xc2), (0xab, 0xb2, 0xbf),
    (0x5c, 0x63, 0x70), (0xff, 0x7b, 0x86), (0xb5, 0xe8, 0x90), (0xff, 0xd6, 0x8a),
    (0x8c, 0xc8, 0xff), (0xe0, 0x9f, 0xf5), (0x7f, 0xdb, 0xe6), (0xff, 0xff, 0xff),
];

fn rgb(c: Color, is_fg: bool) -> (u8, u8, u8) {
    let ansi = |i: usize| ANSI[i];
    match c {
        Color::Reset => {
            if is_fg {
                DEFAULT_FG
            } else {
                DEFAULT_BG
            }
        }
        Color::Indexed(i @ 0..=15) => ansi(usize::from(i)),
        Color::Black => ansi(0),
        Color::Red => ansi(1),
        Color::Green => ansi(2),
        Color::Yellow => ansi(3),
        Color::Blue => ansi(4),
        Color::Magenta => ansi(5),
        Color::Cyan => ansi(6),
        Color::Gray => ansi(7),
        Color::DarkGray => ansi(8),
        Color::LightRed => ansi(9),
        Color::LightGreen => ansi(10),
        Color::LightYellow => ansi(11),
        Color::LightBlue => ansi(12),
        Color::LightMagenta => ansi(13),
        Color::LightCyan => ansi(14),
        Color::White => ansi(15),
        other => crate::tui::theme::to_rgb(other).unwrap_or(DEFAULT_FG),
    }
}

fn hex(c: (u8, u8, u8)) -> String {
    format!("#{:02x}{:02x}{:02x}", c.0, c.1, c.2)
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// Box drawing as (left, right, up, down) weights (1 light, 2 heavy), dashes per cell (0 =
/// solid), and whether it's a rounded corner.
fn box_char(c: char) -> Option<([u8; 4], u8, bool)> {
    let s = |l, r, u, d| Some(([l, r, u, d], 0, false));
    let dashed = |w: [u8; 4], n| Some((w, n, false));
    let round = |w: [u8; 4]| Some((w, 0, true));
    match c {
        '─' => s(1, 1, 0, 0),
        '│' => s(0, 0, 1, 1),
        '┌' => s(0, 1, 0, 1),
        '┐' => s(1, 0, 0, 1),
        '└' => s(0, 1, 1, 0),
        '┘' => s(1, 0, 1, 0),
        '├' => s(0, 1, 1, 1),
        '┤' => s(1, 0, 1, 1),
        '┬' => s(1, 1, 0, 1),
        '┴' => s(1, 1, 1, 0),
        '┼' => s(1, 1, 1, 1),
        '━' => s(2, 2, 0, 0),
        '┃' => s(0, 0, 2, 2),
        '┏' => s(0, 2, 0, 2),
        '┓' => s(2, 0, 0, 2),
        '┗' => s(0, 2, 2, 0),
        '┛' => s(2, 0, 2, 0),
        '┣' => s(0, 2, 2, 2),
        '┫' => s(2, 0, 2, 2),
        '┳' => s(2, 2, 0, 2),
        '┻' => s(2, 2, 2, 0),
        '╋' => s(2, 2, 2, 2),
        '╴' => s(1, 0, 0, 0),
        '╵' => s(0, 0, 1, 0),
        '╶' => s(0, 1, 0, 0),
        '╷' => s(0, 0, 0, 1),
        '╸' => s(2, 0, 0, 0),
        '╹' => s(0, 0, 2, 0),
        '╺' => s(0, 2, 0, 0),
        '╻' => s(0, 0, 0, 2),
        '╌' => dashed([1, 1, 0, 0], 2),
        '╍' => dashed([2, 2, 0, 0], 2),
        '┄' => dashed([1, 1, 0, 0], 3),
        '┅' => dashed([2, 2, 0, 0], 3),
        '┈' => dashed([1, 1, 0, 0], 4),
        '┉' => dashed([2, 2, 0, 0], 4),
        '╎' => dashed([0, 0, 1, 1], 2),
        '╏' => dashed([0, 0, 2, 2], 2),
        '┆' => dashed([0, 0, 1, 1], 3),
        '┇' => dashed([0, 0, 2, 2], 3),
        '┊' => dashed([0, 0, 1, 1], 4),
        '┋' => dashed([0, 0, 2, 2], 4),
        '╭' => round([0, 1, 0, 1]),
        '╮' => round([1, 0, 0, 1]),
        '╰' => round([0, 1, 1, 0]),
        '╯' => round([1, 0, 1, 0]),
        _ => None,
    }
}

fn draw_box(out: &mut String, x: f64, y: f64, col: &str, w: [u8; 4], dashes: u8, round: bool) {
    let (cx, cy) = (x + CW / 2.0, y + CH / 2.0);
    let sw = |k: u8| if k == 2 { 2.6 } else { 1.15 };
    if round {
        let r = CW / 2.0;
        let (hx, vy) = (if w[0] > 0 { x } else { x + CW }, if w[2] > 0 { y } else { y + CH });
        let (ax, ay) = (if w[0] > 0 { cx - r } else { cx + r }, if w[2] > 0 { cy - r } else { cy + r });
        let _ = write!(
            out,
            r#"<path d="M{hx:.2},{cy:.2} L{ax:.2},{cy:.2} Q{cx:.2},{cy:.2} {cx:.2},{ay:.2} L{cx:.2},{vy:.2}" fill="none" stroke="{col}" stroke-width="1.15"/>"#
        );
        return;
    }
    let dash = |len: f64| {
        if dashes == 0 {
            String::new()
        } else {
            let p = len / f64::from(dashes);
            format!(r#" stroke-dasharray="{:.2} {:.2}" stroke-dashoffset="{:.2}""#, p * 0.55, p * 0.45, -p * 0.225)
        }
    };
    let hor = w[0] == w[1] && w[0] > 0;
    let ver = w[2] == w[3] && w[2] > 0;
    // A straight line: one stroke across the cell.
    if hor && (dashes > 0 || w[2] == 0 && w[3] == 0) {
        let _ = write!(out, r#"<line x1="{x:.2}" y1="{cy:.2}" x2="{:.2}" y2="{cy:.2}" stroke="{col}" stroke-width="{}"{}/>"#, x + CW, sw(w[0]), dash(CW));
        return;
    }
    if ver && (dashes > 0 || w[0] == 0 && w[1] == 0) {
        let _ = write!(out, r#"<line x1="{cx:.2}" y1="{y:.2}" x2="{cx:.2}" y2="{:.2}" stroke="{col}" stroke-width="{}"{}/>"#, y + CH, sw(w[2]), dash(CH));
        return;
    }
    // Junctions and corners: an arm from the centre to each edge, overlapping by half a stroke
    // so corners close.
    let ext = sw(*w.iter().max().unwrap_or(&1)) / 2.0;
    let arms = [(x, cy, cx + ext, cy), (x + CW, cy, cx - ext, cy), (cx, y, cx, cy + ext), (cx, y + CH, cx, cy - ext)];
    for (i, (ex, ey, sx, sy)) in arms.iter().enumerate() {
        if w[i] > 0 {
            let _ = write!(out, r#"<line x1="{sx:.2}" y1="{sy:.2}" x2="{ex:.2}" y2="{ey:.2}" stroke="{col}" stroke-width="{}"/>"#, sw(w[i]));
        }
    }
}

/// Part of a cell, (x0, y0, x1, y1) as fractions of it, and an opacity.
type CellRect = (f64, f64, f64, f64, f64);

/// Block elements U+2580–U+259F as rects.
fn block_rects(c: char) -> Option<Vec<CellRect>> {
    let u = c as u32;
    let r = |a, b, c, d| (a, b, c, d, 1.0);
    Some(match u {
        0x2580 => vec![r(0.0, 0.0, 1.0, 0.5)],
        0x2581..=0x2588 => {
            let f = f64::from(u - 0x2580) / 8.0;
            vec![r(0.0, 1.0 - f, 1.0, 1.0)]
        }
        0x2589..=0x258F => {
            let f = f64::from(0x2590 - u) / 8.0;
            vec![r(0.0, 0.0, f, 1.0)]
        }
        0x2590 => vec![r(0.5, 0.0, 1.0, 1.0)],
        0x2591 => vec![(0.0, 0.0, 1.0, 1.0, 0.25)],
        0x2592 => vec![(0.0, 0.0, 1.0, 1.0, 0.5)],
        0x2593 => vec![(0.0, 0.0, 1.0, 1.0, 0.75)],
        0x2594 => vec![r(0.0, 0.0, 1.0, 0.125)],
        0x2595 => vec![r(0.875, 0.0, 1.0, 1.0)],
        0x2596..=0x259F => {
            // Quadrants, as bits: upper left, upper right, lower left, lower right.
            let q: u8 = match u {
                0x2596 => 0b0010,
                0x2597 => 0b0001,
                0x2598 => 0b1000,
                0x2599 => 0b1011,
                0x259A => 0b1001,
                0x259B => 0b1110,
                0x259C => 0b1101,
                0x259D => 0b0100,
                0x259E => 0b0110,
                _ => 0b0111,
            };
            [(8, r(0.0, 0.0, 0.5, 0.5)), (4, r(0.5, 0.0, 1.0, 0.5)), (2, r(0.0, 0.5, 0.5, 1.0)), (1, r(0.5, 0.5, 1.0, 1.0))]
                .into_iter()
                .filter(|(bit, _)| q & bit != 0)
                .map(|(_, rect)| rect)
                .collect()
        }
        _ => return None,
    })
}

/// A buffer as an SVG picture of the terminal.
pub fn svg(buf: &Buffer) -> String {
    let area = buf.area;
    let (w, h) = (f64::from(area.width) * CW, f64::from(area.height) * CH);
    let mut bg = String::new();
    let mut shapes = String::new();
    let mut text = String::new();
    let cell_bg = |x: u16, y: u16| {
        let c = &buf[(area.x + x, area.y + y)];
        if c.modifier.contains(Modifier::REVERSED) { rgb(c.fg, true) } else { rgb(c.bg, false) }
    };
    for y in 0..area.height {
        let py = f64::from(y) * CH;
        // Background runs.
        let mut x = 0;
        while x < area.width {
            let c = cell_bg(x, y);
            let start = x;
            while x < area.width && cell_bg(x, y) == c {
                x += 1;
            }
            if c != DEFAULT_BG {
                let _ = write!(bg, r#"<rect x="{:.2}" y="{py:.2}" width="{:.2}" height="{CH}" fill="{}"/>"#, f64::from(start) * CW, f64::from(x - start) * CW, hex(c));
            }
        }
        // Glyphs, in runs of the same style.
        let mut run: Option<(String, Vec<f64>, String)> = None; // (attributes, xs, text)
        let flush = |run: &mut Option<(String, Vec<f64>, String)>, text: &mut String| {
            if let Some((attrs, xs, s)) = run.take() {
                let xs: Vec<String> = xs.iter().map(|v| format!("{v:.2}")).collect();
                let _ = write!(text, r#"<text x="{}" y="{:.2}"{attrs}>{}</text>"#, xs.join(" "), py + CH * 0.74, esc(&s));
            }
        };
        let mut x = 0u16;
        while x < area.width {
            let cell = &buf[(area.x + x, area.y + y)];
            let sym = cell.symbol();
            let m = cell.modifier;
            let rev = m.contains(Modifier::REVERSED);
            let mut col = if rev { rgb(cell.bg, false) } else { rgb(cell.fg, true) };
            if m.contains(Modifier::DIM) {
                let b = cell_bg(x, y);
                let mix = |a: u8, b: u8| ((u16::from(a) * 6 + u16::from(b) * 4) / 10) as u8;
                col = (mix(col.0, b.0), mix(col.1, b.1), mix(col.2, b.2));
            }
            let px = f64::from(x) * CW;
            let colh = hex(col);
            let ch = sym.chars().next().unwrap_or(' ');
            let width = sym.width().max(1) as u16;
            if sym.trim().is_empty() {
                flush(&mut run, &mut text);
                x += 1;
                continue;
            }
            if sym.chars().count() == 1 {
                if let Some(rects) = block_rects(ch) {
                    flush(&mut run, &mut text);
                    for (a, b, c, d, o) in rects {
                        let op = if o < 1.0 { format!(r#" fill-opacity="{o}""#) } else { String::new() };
                        let _ = write!(shapes, r#"<rect x="{:.2}" y="{:.2}" width="{:.2}" height="{:.2}" fill="{colh}"{op}/>"#, px + a * CW, py + b * CH, (c - a) * CW, (d - b) * CH);
                    }
                    x += 1;
                    continue;
                }
                if let Some((wts, dashes, round)) = box_char(ch) {
                    flush(&mut run, &mut text);
                    draw_box(&mut shapes, px, py, &colh, wts, dashes, round);
                    x += 1;
                    continue;
                }
                if (0x2800..=0x28FF).contains(&(ch as u32)) {
                    flush(&mut run, &mut text);
                    let bits = ch as u32 - 0x2800;
                    let dots = [(0, 0), (0, 1), (0, 2), (1, 0), (1, 1), (1, 2), (0, 3), (1, 3)];
                    for (i, (dx, dy)) in dots.iter().enumerate() {
                        if bits & (1 << i) != 0 {
                            let _ = write!(shapes, r#"<circle cx="{:.2}" cy="{:.2}" r="1.25" fill="{colh}"/>"#, px + CW * (0.3 + 0.4 * f64::from(*dx)), py + CH * (0.14 + 0.24 * f64::from(*dy)));
                        }
                    }
                    x += 1;
                    continue;
                }
            }
            let mut attrs = format!(r#" fill="{colh}""#);
            if m.contains(Modifier::BOLD) {
                attrs.push_str(r#" font-weight="700""#);
            }
            if m.contains(Modifier::ITALIC) {
                attrs.push_str(r#" font-style="italic""#);
            }
            if m.contains(Modifier::UNDERLINED) {
                attrs.push_str(r#" text-decoration="underline""#);
            }
            if width > 1 {
                // A wide glyph, centred across its cells.
                flush(&mut run, &mut text);
                let _ = write!(text, r#"<text x="{:.2}" y="{:.2}" text-anchor="middle"{attrs}>{}</text>"#, px + CW * f64::from(width) / 2.0, py + CH * 0.74, esc(sym));
                x += width;
                continue;
            }
            match &mut run {
                Some((a, xs, s)) if *a == attrs => {
                    xs.push(px);
                    s.push_str(sym);
                }
                _ => {
                    flush(&mut run, &mut text);
                    run = Some((attrs, vec![px], sym.to_string()));
                }
            }
            x += 1;
        }
        flush(&mut run, &mut text);
    }
    format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">
<rect width="100%" height="100%" fill="{}"/>
<g shape-rendering="crispEdges">{bg}</g>
<g>{shapes}</g>
<g font-family="'JetBrainsMono Nerd Font Mono','JetBrains Mono','DejaVu Sans Mono',Menlo,Consolas,monospace" font-size="{FONT}" xml:space="preserve">{text}</g>
</svg>
"#,
        hex(DEFAULT_BG)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_names() {
        assert_eq!(key("q"), Some(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)));
        assert_eq!(key("ctrl+k"), Some(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL)));
        assert_eq!(key("shift+tab").map(|k| k.code), Some(KeyCode::BackTab));
        assert_eq!(key("F1").map(|k| k.code), Some(KeyCode::F(1)));
        assert_eq!(key("comma").map(|k| k.code), Some(KeyCode::Char(',')));
        assert_eq!(key("f13"), None);
        assert_eq!(key("bogus"), None);
    }

    #[test]
    fn options() {
        let args = |s: &str| s.split(' ').map(String::from).collect::<Vec<_>>();
        let s = parse(&args("--shot a.svg --size 80x24 --view finder --keys down,enter")).unwrap();
        assert_eq!((s.width, s.height, s.view, s.keys.len()), (80, 24, ViewId::Finder, 2));
        assert_eq!(s.home, None);
        let s = parse(&args("--shot a.svg --size 80x24 --home /home/u")).unwrap();
        assert_eq!(s.home.as_deref(), Some("/home/u"));
        assert!(parse(&args("--shot a.svg")).unwrap_err().contains("--size"));
        assert!(parse(&args("--shot a.svg --size 80by24")).is_err());
        assert!(parse(&args("--shot a.svg --size 80x24 --view grid")).is_err());
        assert!(parse(&args("--shot a.svg --size 80x24 --keys nope")).is_err());
    }
}
