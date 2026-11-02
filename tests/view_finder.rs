//! Finder on ratatui's `TestBackend`: typing filters and lights up matches, fuzzy queries, Esc,
//! ended sessions while searching, the narrow layout, and a selection that survives a re-sort.

use board::actions::Cursor;
use board::facts::{Snapshot, State};
use board::shot;
use board::tui::app::{App, ViewId};
use board::tui::theme::{Depth, Theme};
use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier};

fn sample() -> Snapshot {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sample.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

/// An app on Finder, nothing typed.
fn finder() -> App {
    let mut a = App::offline(sample(), "/home/u", "/tmp", Theme::default());
    a.view = ViewId::Finder;
    a
}

fn keys(names: &str) -> Vec<ratatui::crossterm::event::KeyEvent> {
    names.split(',').filter(|k| !k.is_empty()).map(|k| shot::key(k).unwrap()).collect()
}

fn press_at(a: &mut App, w: u16, h: u16, names: &str) -> Buffer {
    shot::render(a, w, h, &keys(names))
}

fn press(a: &mut App, names: &str) -> Buffer {
    press_at(a, 160, 45, names)
}

/// Type `text` (letters and spaces only).
fn typing(text: &str) -> String {
    text.chars().map(|c| if c == ' ' { "space".to_string() } else { c.to_string() }).collect::<Vec<_>>().join(",")
}

fn row(buf: &Buffer, y: u16) -> String {
    (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
}

fn screen(buf: &Buffer) -> String {
    (0..buf.area.height).map(|y| row(buf, y) + "\n").collect()
}

/// The left `w` columns of the screen: the results list in the wide layout.
fn left(buf: &Buffer, w: u16) -> String {
    (0..buf.area.height).map(|y| (0..w).map(|x| buf[(x, y)].symbol()).collect::<String>() + "\n").collect()
}

/// The text of the first row lit as the selection (below the header, whose tab is lit too).
fn lit_row(buf: &Buffer, t: &Theme) -> Option<String> {
    (1..buf.area.height).find_map(|y| {
        let lit = (0..buf.area.width).any(|x| [t.sel, t.needs_sel].contains(&buf[(x, y)].bg) && buf[(x, y)].symbol() != " ");
        lit.then(|| row(buf, y))
    })
}

/// Where `word` first appears at or below row 4 (under the search box), and whether each of
/// its cells is drawn as a match (accent, underlined).
fn lit_cells(buf: &Buffer, t: &Theme, word: &str, max_x: u16) -> Option<Vec<bool>> {
    let n = word.chars().count() as u16;
    for y in 4..buf.area.height {
        for x in 0..max_x.saturating_sub(n) {
            let here: String = (0..n).map(|i| buf[(x + i, y)].symbol()).collect();
            if here == word {
                return Some(
                    (0..n)
                        .map(|i| {
                            let c = &buf[(x + i, y)];
                            c.fg == t.accent && c.modifier.contains(Modifier::UNDERLINED)
                        })
                        .collect(),
                );
            }
        }
    }
    None
}

#[test]
fn typing_filters_and_lights_the_matches() {
    let t = Theme::default();
    let mut a = finder();
    let before = press(&mut a, "");
    assert!(screen(&before).contains("10/14"), "ended hidden with nothing typed:\n{}", screen(&before));

    let buf = press(&mut a, &typing("infra"));
    let s = screen(&buf);
    assert!(s.contains("› infra"), "{s}");
    assert!(s.contains("4/14"), "{s}");
    let list = left(&buf, 64);
    for title in ["Approve: kubectl rollout restart", "Terraform plan", "Postgres failover drill", "Rotate the staging certs"] {
        assert!(list.contains(title), "{title} is listed:\n{list}");
    }
    assert!(!list.contains("Fix the flaky auth test"), "{list}");
    // The project name lights up in the list, the path and folder in the details.
    assert_eq!(lit_cells(&buf, &t, "infra", 64), Some(vec![true; 5]));
    assert!(s.contains("~/work/infra"), "{s}");
}

#[test]
fn fuzzy_flky_finds_flaky_and_lights_each_letter() {
    let t = Theme::default();
    let mut a = finder();
    let buf = press(&mut a, &typing("flky"));
    let list = left(&buf, 64);
    assert!(list.contains("Fix the flaky auth test"), "{list}");
    assert!(screen(&buf).contains("1/14"));
    assert_eq!(lit_cells(&buf, &t, "flaky", 64), Some(vec![true, true, false, true, true]));
    assert_eq!(a.cursor, Some(Cursor::Session("a1".into())), "a new query puts the cursor on its best result");
}

#[test]
fn esc_clears_the_query_then_goes_back() {
    let mut a = App::offline(sample(), "/home/u", "/tmp", Theme::default());
    press(&mut a, "/");
    assert_eq!(a.view, ViewId::Finder);
    let buf = press(&mut a, &typing("api"));
    assert!(screen(&buf).contains("› api"));
    let buf = press(&mut a, "esc");
    assert_eq!(a.view, ViewId::Finder, "the first Esc clears");
    assert!(screen(&buf).contains("Search titles, projects and folders"), "{}", screen(&buf));
    press(&mut a, "esc");
    assert_eq!(a.view, ViewId::Columns, "the second goes back");
    assert!(!a.quit);
}

#[test]
fn ended_sessions_join_in_while_a_query_is_typed() {
    let mut a = finder();
    let s = screen(&press(&mut a, ""));
    assert!(!s.contains("Rate limiter design") && !s.contains("ENDED"), "{s}");
    let s = screen(&press(&mut a, &typing("rate")));
    assert!(s.contains("Rate limiter design") && s.contains("ENDED"), "{s}");
    assert!(s.contains("ended shown"));
    let s = screen(&press(&mut a, "ctrl+u"));
    assert!(!s.contains("Rate limiter design"), "gone again with the query: {s}");
    // Ctrl+E (not a letter, which is text) turns them on for good.
    let s = screen(&press(&mut a, "ctrl+e"));
    assert!(a.prefs.show_ended);
    assert!(s.contains("Rate limiter design"));
}

#[test]
fn letters_are_text_and_ctrl_keys_move() {
    let mut a = finder();
    a.cursor = Some(Cursor::Session("a1".into()));
    press(&mut a, "q,s,x,e,n");
    assert!(!a.quit && a.overlay.is_none() && !a.prefs.show_ended);
    press(&mut a, "ctrl+u,ctrl+n,ctrl+n");
    let after_two = a.cursor.clone();
    press(&mut a, "ctrl+p");
    assert_ne!(a.cursor, after_two);
    press(&mut a, "end");
    assert_eq!(a.cursor, Some(Cursor::Session("i2".into())), "the last row: the stale one");
    press(&mut a, "home");
    assert_eq!(a.cursor, Some(Cursor::Session("a1".into())));
    press(&mut a, "pgdn");
    assert_ne!(a.cursor, Some(Cursor::Session("a1".into())));
}

#[test]
fn enter_jumps_to_the_selection_and_does_nothing_without_a_match() {
    let mut a = finder();
    press(&mut a, &typing("terraform"));
    a.handle_key(shot::key("enter").unwrap());
    assert_eq!(a.take_effects().len(), 1);
    assert_eq!(a.cursor, Some(Cursor::Session("i1".into())));

    press(&mut a, &typing("zzqx"));
    a.handle_key(shot::key("enter").unwrap());
    assert!(a.take_effects().is_empty(), "nothing listed, nothing to jump into");
    a.handle_key(shot::key("ctrl+x").unwrap());
    assert!(a.overlay.is_none());
    assert!(a.toasts().any(|t| t.text.contains("Nothing matches")));
}

#[test]
fn the_preview_shows_the_chat_the_wait_and_the_gauges() {
    let mut a = finder();
    a.cursor = Some(Cursor::Session("i3".into()));
    let s = screen(&press(&mut a, ""));
    assert!(s.contains("WAITING ON YOU"), "{s}");
    assert!(s.contains("$ kubectl -n edge rollout restart deploy/ingress"), "{s}");
    assert!(s.contains("Roll the ingress pods"), "your ask: {s}");
    assert!(s.contains("31k of 1M"), "context, labelled with its scale: {s}");
    assert!(s.contains("job e5f6a7b8"));

    a.cursor = Some(Cursor::Session("d1".into()));
    let s = screen(&press(&mut a, ""));
    assert!(s.contains("RUNNING") && s.contains("Rewriting the retention query"), "{s}");
}

#[test]
fn narrow_stacks_the_preview_under_a_one_line_list() {
    let mut a = finder();
    a.cursor = Some(Cursor::Session("i3".into()));
    let buf = press_at(&mut a, 80, 40, "");
    let s = screen(&buf);
    let results = s.lines().position(|l| l.contains("results")).unwrap();
    let preview = s.lines().position(|l| l.contains("preview")).unwrap();
    assert!(preview > results + 5, "{s}");
    let line = s.lines().find(|l| l.contains("Approve: kubectl rollout restart")).unwrap();
    assert!(line.contains("infra") && line.contains("42m"), "title, project and age on one line: {line}");
    assert!(s.contains("WAITING ON YOU"), "{s}");
}

#[test]
fn the_selection_survives_a_re_sort() {
    let t = Theme::default();
    let mut a = finder();
    a.cursor = Some(Cursor::Session("d2".into()));
    let before = lit_row(&press(&mut a, ""), &t).unwrap();
    assert!(before.contains("dbt test failures"), "{before}");

    let mut snap = sample();
    snap.sessions.reverse();
    snap.sessions.iter_mut().find(|s| s.session_id == "d2").unwrap().state = State::NeedsYou;
    snap.generation += 1;
    a.on_collect(Ok(snap));
    let buf = press(&mut a, "");
    let after = lit_row(&buf, &t).unwrap();
    assert!(after.contains("dbt test failures"), "{after}");
    assert_eq!(a.cursor, Some(Cursor::Session("d2".into())));

    // With a query typed too: "d" keeps several rows, then the snapshot changes again.
    press(&mut a, "ctrl+u,d,b,t");
    assert_eq!(a.cursor, Some(Cursor::Session("d2".into())));
    let mut snap = sample();
    snap.sessions.rotate_left(5);
    a.on_collect(Ok(snap));
    assert!(lit_row(&press(&mut a, ""), &t).unwrap().contains("dbt test failures"));
}

#[test]
fn no_true_colour_leaks_into_a_256_colour_terminal() {
    for (w, h, sel) in [(160, 45, "i3"), (80, 40, "d1"), (160, 45, "t1")] {
        let mut a = App::offline(sample(), "/home/u", "/tmp", Theme::new(Depth::Indexed256));
        a.view = ViewId::Finder;
        a.cursor = Some(Cursor::Session(sel.into()));
        let buf = press_at(&mut a, w, h, "c,s,v,ctrl+u");
        for c in buf.content() {
            assert!(!matches!(c.fg, Color::Rgb(..)) && !matches!(c.bg, Color::Rgb(..)), "{c:?}");
        }
    }
}

#[test]
fn empty_and_tiny_screens_draw() {
    for (w, h) in [(30, 8), (40, 12), (60, 14), (109, 30), (110, 30), (240, 70)] {
        for snap in [sample(), Snapshot::default()] {
            let mut a = App::offline(snap, "/home/u", "/tmp", Theme::default());
            a.view = ViewId::Finder;
            press_at(&mut a, w, h, "a,down,ctrl+k,esc,z,z,enter,esc,esc");
        }
    }
    let mut a = App::offline(Snapshot::default(), "/home/u", "/tmp", Theme::default());
    a.view = ViewId::Finder;
    let s = screen(&press(&mut a, ""));
    assert!(s.contains("No sessions yet"), "{s}");
}
