//! The one palette every view draws with: Columns' structure (a dark board that paints its own
//! background, so a light terminal still renders), with **amber** for "needs you", the colour
//! Claude Code and the mod's band use for "your turn".
//!
//! True colour, degraded once at construction to the xterm 256-colour palette when the terminal
//! doesn't say it has true colour (`COLORTERM` isn't `truecolor`/`24bit`). `SESSION_BOARD_COLOR=
//! truecolor|256` overrides the guess (tmux often hides `COLORTERM`; Apple Terminal has no true
//! colour). Colours computed while drawing (gradients, tints) go through [`Theme::mix`] and
//! [`Theme::darken`] so they degrade too.

use crate::facts::State;
use ratatui::style::{Color, Modifier, Style};

/// How many colours the terminal shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Depth {
    TrueColor,
    /// The xterm 256-colour palette; only indices 16–255, since 0–15 are whatever the
    /// terminal's own theme says.
    Indexed256,
}

impl Depth {
    /// From this process's environment.
    pub fn detect() -> Depth {
        Depth::from_vars(
            std::env::var("SESSION_BOARD_COLOR").ok().as_deref(),
            std::env::var("COLORTERM").ok().as_deref(),
        )
    }

    /// `choice` is `SESSION_BOARD_COLOR`, `colorterm` is `COLORTERM`.
    pub fn from_vars(choice: Option<&str>, colorterm: Option<&str>) -> Depth {
        let lower = |v: Option<&str>| v.map(|s| s.trim().to_ascii_lowercase());
        match lower(choice).as_deref() {
            Some("truecolor" | "24bit") => return Depth::TrueColor,
            Some("256") => return Depth::Indexed256,
            _ => {}
        }
        match lower(colorterm).as_deref() {
            Some("truecolor" | "24bit") => Depth::TrueColor,
            _ => Depth::Indexed256,
        }
    }
}

/// Declares [`Theme`]'s colours once: each field, its true-colour value and what it is for.
macro_rules! palette {
    ($( $(#[$doc:meta])* $name:ident = ($r:expr, $g:expr, $b:expr), )*) => {
        /// The board's colours, already degraded to what the terminal can show.
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct Theme {
            pub depth: Depth,
            $( $(#[$doc])* pub $name: Color, )*
        }

        impl Theme {
            pub fn new(depth: Depth) -> Theme {
                Theme { depth, $( $name: degrade(Color::Rgb($r, $g, $b), depth), )* }
            }
        }
    };
}

palette! {
    // Surfaces, darkest to lightest.
    /// Shadows under popups and toasts.
    shadow = (6, 8, 11),
    /// The board's background.
    bg = (13, 17, 23),
    /// A recessed strip (metadata, alternate rows).
    strip = (18, 23, 30),
    /// Header and footer bars.
    bar = (22, 27, 34),
    /// The selected row when its column doesn't have focus.
    sel_blur = (24, 29, 37),
    /// Popups, toasts, dialogs.
    raised = (26, 32, 42),
    /// An empty gauge or scrollbar track.
    track = (28, 33, 40),
    /// The selected row in the focused column.
    sel = (30, 40, 62),
    /// Keycap chips (`↵`, `q`).
    keycap = (38, 46, 62),
    /// Separators and hairlines.
    sep = (40, 46, 54),
    /// Popup and panel borders.
    border = (56, 64, 82),

    // Text, faintest to brightest.
    faint = (84, 92, 102),
    dim = (125, 133, 144),
    fg = (201, 209, 217),
    bright = (240, 246, 252),
    /// Focus, the brand chip, links.
    accent = (122, 162, 247),

    // States.
    /// Needs you: amber.
    needs = (255, 178, 36),
    /// Text on an amber fill.
    needs_ink = (29, 20, 5),
    /// A needs-you row or panel's background.
    needs_bg = (38, 29, 16),
    /// A selected needs-you row.
    needs_sel = (58, 42, 18),
    running = (86, 211, 100),
    elsewhere = (57, 197, 207),
    idle = (150, 158, 168),
    /// Not running although its state says otherwise: a muted rose, apart from amber.
    stale = (224, 108, 117),
    stale_bg = (44, 24, 27),
    ended = (100, 108, 118),

    // Conversation.
    you_bg = (28, 45, 78),
    claude = (217, 160, 120),
    claude_bg = (29, 34, 43),
    /// Commands and code in a tool-call panel.
    code = (255, 200, 140),

    // Feedback.
    ok = (86, 211, 100),
    warn = (255, 178, 36),
    error = (248, 81, 73),
}

impl Default for Theme {
    fn default() -> Self {
        Theme::new(Depth::TrueColor)
    }
}

impl Theme {
    /// From this process's environment ([`Depth::detect`]).
    pub fn detect() -> Theme {
        Theme::new(Depth::detect())
    }

    /// A state's colour: the glyph, the label, the bar.
    pub fn state(&self, s: State) -> Color {
        match s {
            State::NeedsYou => self.needs,
            State::Running => self.running,
            State::Elsewhere => self.elsewhere,
            State::Idle => self.idle,
            State::Stale => self.stale,
            State::Ended => self.ended,
        }
    }

    /// `a` moved towards `b` by `t` (0 = `a`, 1 = `b`), degraded for this terminal.
    pub fn mix(&self, a: Color, b: Color, t: f32) -> Color {
        degrade(mix(a, b, t), self.depth)
    }

    /// `c` scaled towards black by `f` (1 = unchanged), degraded for this terminal.
    pub fn darken(&self, c: Color, f: f32) -> Color {
        degrade(darken(c, f), self.depth)
    }

    /// A state's colour faded into the background: the fill behind a state pill.
    pub fn tint(&self, s: State) -> Color {
        self.mix(self.state(s), self.bg, 0.78)
    }

    /// The context gauge's colour at position `t` along it: green, amber, red.
    pub fn gauge(&self, t: f32) -> Color {
        if t < 0.5 {
            self.mix(self.running, self.needs, t * 2.0)
        } else {
            self.mix(self.needs, self.error, (t - 0.5) * 2.0)
        }
    }

    /// Plain text on the board.
    pub fn text(&self) -> Style {
        Style::new().fg(self.fg).bg(self.bg)
    }

    /// Bold `c`.
    pub fn bold(&self, c: Color) -> Style {
        Style::new().fg(c).add_modifier(Modifier::BOLD)
    }
}

/// `Style::new().fg(c)`, the commonest style.
pub fn fg(c: Color) -> Style {
    Style::new().fg(c)
}

/// A colour as RGB: true-colour as is, an xterm index from its palette. `None` for the 16
/// terminal-themed colours and `Reset`, which have no fixed value.
pub fn to_rgb(c: Color) -> Option<(u8, u8, u8)> {
    match c {
        Color::Rgb(r, g, b) => Some((r, g, b)),
        Color::Indexed(i @ 16..=231) => {
            let i = i - 16;
            Some((LEVELS[(i / 36) as usize], LEVELS[(i / 6 % 6) as usize], LEVELS[(i % 6) as usize]))
        }
        Color::Indexed(i @ 232..=255) => {
            let g = 8 + (i - 232) * 10;
            Some((g, g, g))
        }
        _ => None,
    }
}

/// `a` moved towards `b` by `t`, in true colour. Colours without a fixed value come back as `a`.
pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let (Some((r1, g1, b1)), Some((r2, g2, b2))) = (to_rgb(a), to_rgb(b)) else { return a };
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color::Rgb(l(r1, r2), l(g1, g2), l(b1, b2))
}

/// `c` scaled towards black by `f`, in true colour.
pub fn darken(c: Color, f: f32) -> Color {
    let Some((r, g, b)) = to_rgb(c) else { return c };
    let s = |x: u8| (f32::from(x) * f.clamp(0.0, 1.0)).round() as u8;
    Color::Rgb(s(r), s(g), s(b))
}

/// The xterm cube's channel levels.
const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// The nearest colour the terminal can show: unchanged in true colour, else the nearest of the
/// 6x6x6 cube and the grey ramp.
pub fn degrade(c: Color, depth: Depth) -> Color {
    let (Depth::Indexed256, Color::Rgb(r, g, b)) = (depth, c) else { return c };
    let near = |v: u8| {
        (0..6).min_by_key(|&i| (i32::from(LEVELS[i]) - i32::from(v)).abs()).unwrap_or(0)
    };
    let (ri, gi, bi) = (near(r), near(g), near(b));
    let cube = 16 + 36 * ri as u8 + 6 * gi as u8 + bi as u8;
    let avg = (u16::from(r) + u16::from(g) + u16::from(b)) / 3;
    let grey = 232 + ((avg.saturating_sub(3)) / 10).min(23) as u8;
    let dist = |i: u8| {
        let (r2, g2, b2) = to_rgb(Color::Indexed(i)).unwrap_or((0, 0, 0));
        let d = |x: u8, y: u8| (i32::from(x) - i32::from(y)).pow(2);
        d(r, r2) + d(g, g2) + d(b, b2)
    };
    Color::Indexed(if dist(grey) < dist(cube) { grey } else { cube })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn depth_follows_the_override_then_colorterm() {
        assert_eq!(Depth::from_vars(None, Some("truecolor")), Depth::TrueColor);
        assert_eq!(Depth::from_vars(None, Some("24bit")), Depth::TrueColor);
        assert_eq!(Depth::from_vars(None, None), Depth::Indexed256);
        assert_eq!(Depth::from_vars(Some("256"), Some("truecolor")), Depth::Indexed256);
        assert_eq!(Depth::from_vars(Some("TrueColor"), None), Depth::TrueColor);
    }

    #[test]
    fn a_256_colour_theme_has_no_rgb_left() {
        let t = Theme::new(Depth::Indexed256);
        for c in [t.bg, t.needs, t.running, t.fg, t.mix(t.running, t.needs, 0.5), t.gauge(0.9)] {
            assert!(matches!(c, Color::Indexed(16..=255)), "{c:?}");
        }
        assert_eq!(degrade(Color::Rgb(0, 0, 0), Depth::Indexed256), Color::Indexed(16));
        assert_eq!(degrade(Color::Rgb(255, 255, 255), Depth::Indexed256), Color::Indexed(231));
        assert_eq!(degrade(Color::Rgb(128, 128, 128), Depth::Indexed256), Color::Indexed(244));
    }

    #[test]
    fn mixing_goes_between_the_two() {
        assert_eq!(mix(Color::Rgb(0, 0, 0), Color::Rgb(200, 100, 50), 0.5), Color::Rgb(100, 50, 25));
        assert_eq!(darken(Color::Rgb(200, 100, 50), 0.5), Color::Rgb(100, 50, 25));
        assert_eq!(mix(Color::Reset, Color::Rgb(1, 2, 3), 0.5), Color::Reset);
    }
}
