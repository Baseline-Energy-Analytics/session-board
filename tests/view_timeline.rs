//! The Timeline view on `TestBackend`: bars and the needs-you marker placed by time on the 24h
//! and 7d axes, the "◂ 3d ago" label, the activity sparkline, the 80-column layout, its keys,
//! and the cursor through a re-sort. Data is made up here, with spans; nothing reads `~/.claude`.

use board::actions::{Cursor, Enter};
use board::claude::transcript::Span;
use board::facts::{Session, Snapshot, State};
use board::shot;
use board::tui::app::{App, Effect, ViewId};
use board::tui::theme::Theme;
use ratatui::buffer::Buffer;

const NOW: f64 = 1_793_550_000.0;
const H: f64 = 3600.0;
const D: f64 = 86_400.0;
/// The track's first column at 160 wide: a 40-cell label column and a space.
const TX: u16 = 41;

fn session(id: &str, title: &str, project: &str, state: State, last: f64, spans: &[(f64, f64)]) -> Session {
    let mut s = Session::new(id, title, project, project, state, last);
    s.spans = spans.iter().map(|&(start, end)| Span { start, end }).collect();
    s.job = Some(format!("{id}000000"));
    s
}

fn snapshot() -> Snapshot {
    let mut wait = session("w", "Approve the deploy", "/home/u/api", State::NeedsYou, NOW - 6.0 * H, &[(NOW - 8.0 * H, NOW - 6.0 * H)]);
    wait.waiting = Some("Bash: deploy --prod".into());
    wait.waiting_since = Some(NOW - 6.0 * H);
    let sessions = vec![
        wait,
        session("r", "Build the thing", "/home/u/api", State::Running, NOW - 60.0, &[(NOW - 2.0 * H, NOW - 60.0)]),
        session("old", "Three days back", "/home/u/web", State::Idle, NOW - 3.0 * D, &[(NOW - 3.0 * D - H, NOW - 3.0 * D)]),
        session("i", "Earlier today", "/home/u/web", State::Idle, NOW - 10.0 * H, &[(NOW - 12.0 * H, NOW - 10.0 * H)]),
        // Hidden (ended), but its work still counts in the sparkline.
        session("e", "Ended one", "/home/u/web", State::Ended, NOW - 20.0 * H, &[(NOW - 21.0 * H, NOW - 20.0 * H)]),
    ];
    Snapshot { now: NOW, generation: 1, sessions }
}

fn app(snap: Snapshot) -> App {
    let mut a = App::offline(snap, "/home/u", "/tmp", Theme::default());
    a.view = ViewId::Timeline;
    a
}

fn press(a: &mut App, w: u16, h: u16, keys: &str) -> Buffer {
    let keys: Vec<_> = keys.split(',').filter(|k| !k.is_empty()).map(|k| shot::key(k).unwrap()).collect();
    shot::render(a, w, h, &keys)
}

fn line(buf: &Buffer, y: u16) -> String {
    (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
}

fn screen(buf: &Buffer) -> String {
    (0..buf.area.height).map(|y| line(buf, y) + "\n").collect()
}

/// The row whose text contains `needle`.
fn row_of(buf: &Buffer, needle: &str) -> u16 {
    (0..buf.area.height).find(|&y| line(buf, y).contains(needle)).unwrap_or_else(|| panic!("no {needle:?} in\n{}", screen(buf)))
}

/// The columns on row `y` (from `from` on) showing `sym`.
fn cols(buf: &Buffer, y: u16, from: u16, sym: &str) -> Vec<u16> {
    (from..buf.area.width).filter(|&x| buf[(x, y)].symbol() == sym).collect()
}

/// The "now" line's column: the bright ┬ on the ruler.
fn now_x(buf: &Buffer) -> u16 {
    let y = row_of(buf, "┬");
    *cols(buf, y, TX, "┬").last().unwrap()
}

/// Where time `t` falls on the track drawn in `buf`, for a range of `secs`.
fn expect_col(buf: &Buffer, t: f64, secs: f64) -> u16 {
    let tw = f64::from(now_x(buf) - TX);
    TX + ((t - (NOW - secs)) / (secs / tw)).floor() as u16
}

#[test]
fn the_needs_you_marker_sits_at_waiting_since_on_both_ranges() {
    let mut a = app(snapshot());
    for (keys, secs, label) in [("", D, "last 24 hours"), ("t", 7.0 * D, "last 7 days")] {
        let buf = press(&mut a, 160, 45, keys);
        assert!(screen(&buf).contains(label), "{}", screen(&buf));
        let y = row_of(&buf, "Approve the deploy");
        let marks = cols(&buf, y, TX, "◆");
        let want = expect_col(&buf, NOW - 6.0 * H, secs);
        assert_eq!(marks.len(), 1, "{}", line(&buf, y));
        assert!(marks[0].abs_diff(want) <= 1, "{label}: ◆ at {}, want {want}: {}", marks[0], line(&buf, y));
        // A dashed line from the marker to now, and the pill saying how long.
        assert!(line(&buf, y).contains("╍"), "{}", line(&buf, y));
        assert!(line(&buf, y).contains("waiting 6h"), "{}", line(&buf, y));
        // The tint: the row is on the needs-you background (lit, since the cursor is on it).
        assert_eq!(buf[(5, y)].bg, Theme::default().needs_sel);
        assert_eq!(buf[(5, row_of(&buf, "Build the thing"))].bg, Theme::default().strip, "the next row is a plain stripe");
    }
}

#[test]
fn bars_are_where_the_spans_are() {
    let mut a = app(snapshot());
    let buf = press(&mut a, 160, 45, "");
    let y = row_of(&buf, "Earlier today");
    let bars: Vec<u16> = (TX..now_x(&buf)).filter(|&x| ["▆", "▄", "▂"].contains(&buf[(x, y)].symbol())).collect();
    assert!(!bars.is_empty(), "{}", line(&buf, y));
    let (lo, hi) = (expect_col(&buf, NOW - 12.0 * H, D), expect_col(&buf, NOW - 10.0 * H, D));
    assert!(bars.iter().all(|x| (lo..=hi).contains(x)), "bars {bars:?} outside {lo}..={hi}");
    assert!(bars.len() >= usize::from(hi - lo), "a two-hour span is a run of cells: {bars:?}");
}

#[test]
fn a_row_with_nothing_in_range_says_when_it_was_last_active() {
    let mut a = app(snapshot());
    let buf = press(&mut a, 160, 45, "");
    let y = row_of(&buf, "Three days back");
    assert!(line(&buf, y).contains("◂ 3d ago"), "{}", line(&buf, y));

    // On the week axis its bar shows instead.
    let buf = press(&mut a, 160, 45, "7");
    let y = row_of(&buf, "Three days back");
    assert!(!line(&buf, y).contains("ago"), "{}", line(&buf, y));
    let want = expect_col(&buf, NOW - 3.0 * D - 0.5 * H, 7.0 * D);
    assert!(["▆", "▄", "▂"].contains(&buf[(want, y)].symbol()), "no bar at {want}: {}", line(&buf, y));
}

#[test]
fn the_sparkline_counts_hidden_ended_sessions_and_says_its_peak() {
    let mut a = app(snapshot());
    assert!(!a.prefs.show_ended);
    let buf = press(&mut a, 160, 45, "");
    assert!(!screen(&buf).contains("Ended one"), "ended rows are hidden");
    let y = row_of(&buf, "activity");
    let x = expect_col(&buf, NOW - 20.5 * H, D);
    assert_ne!(buf[(x, y)].symbol(), " ", "the ended session's work counts: {}", line(&buf, y));
    assert!(line(&buf, y).contains("peak 1"), "{}", line(&buf, y));
}

#[test]
fn t_toggles_the_range_and_1_and_7_pick_one() {
    let mut a = app(snapshot());
    assert!(screen(&press(&mut a, 160, 45, "")).contains("last 24 hours"));
    assert!(screen(&press(&mut a, 160, 45, "t")).contains("last 7 days"));
    assert!(screen(&press(&mut a, 160, 45, "t")).contains("last 24 hours"));
    assert!(screen(&press(&mut a, 160, 45, "7")).contains("last 7 days"));
    assert!(screen(&press(&mut a, 160, 45, "7")).contains("last 7 days"));
    assert!(screen(&press(&mut a, 160, 45, "1")).contains("last 24 hours"));
    assert_eq!(a.view, ViewId::Timeline, "Tab isn't the range key");
}

#[test]
fn at_80_columns_the_state_column_goes_and_ages_move_into_the_labels() {
    let mut a = app(snapshot());
    let wide = press(&mut a, 160, 45, "");
    assert!(line(&wide, row_of(&wide, "Earlier today")).contains("idle · 10h"));
    let buf = press(&mut a, 80, 40, "");
    let text = screen(&buf);
    assert!(!text.contains("idle · "), "{text}");
    assert!(!text.contains("state"), "{text}");
    let row = line(&buf, row_of(&buf, "Earlier today"));
    assert!(row.contains("10h"), "{row}");
    assert!(text.contains("waiting 6h"), "{text}");
    assert!(text.contains("of 1M"), "the context gauge says its scale: {text}");
}

#[test]
fn moving_skips_project_headers_and_enter_jumps() {
    let mut a = app(snapshot());
    a.cursor = Some(Cursor::Session("w".into()));
    press(&mut a, 160, 45, "down,down");
    // w, r (api), then i (web, the most recent there).
    assert_eq!(a.cursor, Some(Cursor::Session("i".into())));
    press(&mut a, 160, 45, "end");
    assert_eq!(a.cursor, Some(Cursor::Session("old".into())));
    press(&mut a, 160, 45, "home");
    assert_eq!(a.cursor, Some(Cursor::Session("w".into())));
    a.handle_key(shot::key("enter").unwrap());
    assert_eq!(a.take_effects(), [Effect::Enter(Enter::Attach { job: "w000000".into() })]);
}

#[test]
fn the_selection_follows_its_session_through_a_re_sort() {
    let t = Theme::default();
    let mut a = app(snapshot());
    a.cursor = Some(Cursor::Session("i".into()));
    let lit = |buf: &Buffer| {
        (1..buf.area.height)
            .find(|&y| [t.sel, t.needs_sel].contains(&buf[(4, y)].bg))
            .map(|y| line(buf, y))
            .unwrap_or_default()
    };
    assert!(lit(&press(&mut a, 160, 45, "")).contains("Earlier today"));

    // A refresh: "i" now needs you, so it and its project move to the top.
    let mut snap = snapshot();
    snap.sessions.reverse();
    let i = snap.sessions.iter_mut().find(|s| s.session_id == "i").unwrap();
    i.state = State::NeedsYou;
    i.last = NOW;
    snap.generation += 1;
    a.on_collect(Ok(snap));
    let buf = press(&mut a, 160, 45, "");
    assert!(lit(&buf).contains("Earlier today"), "{}", screen(&buf));
    assert_eq!(a.cursor, Some(Cursor::Session("i".into())));
    assert!(row_of(&buf, "Earlier today") < row_of(&buf, "Approve the deploy"));
}

#[test]
fn many_rows_scroll_to_keep_the_selection_in_view() {
    let mut snap = snapshot();
    for k in 0..60 {
        snap.sessions.push(session(&format!("x{k}"), &format!("Filler {k:02}"), "/home/u/zz", State::Idle, NOW - 30.0 * D, &[]));
    }
    let mut a = app(snap);
    let buf = press(&mut a, 160, 45, "end");
    assert_eq!(a.cursor, Some(Cursor::Session("x59".into())));
    assert!(screen(&buf).contains("Filler 59"), "{}", screen(&buf));
    let buf = press(&mut a, 160, 45, "home");
    assert!(screen(&buf).contains("Approve the deploy"));
    assert!(screen(&buf).contains("▾ api"), "the first project's header shows too");
}
