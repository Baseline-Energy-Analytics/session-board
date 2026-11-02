//! The terminal app on ratatui's `TestBackend`: key routing, the cursor across refreshes and
//! view switches, every view at every size, and `board --shot`. Nothing here writes prefs or
//! runs `claude` (an offline app has no prefs path, and effects are only inspected).

use board::actions::{Cursor, Enter};
use board::facts::{Snapshot, State};
use board::shot;
use board::tui::app::{App, Effect, ViewId};
use board::tui::overlays::Overlay;
use board::tui::theme::Theme;
use ratatui::buffer::Buffer;
use std::process::Command;

mod common;
use common::TempDir;

fn sample() -> Snapshot {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sample.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn app() -> App {
    App::offline(sample(), "/home/u", "/tmp", Theme::default())
}

/// Press keys by name (`shot::key`), drawing at 160x45 before each as the real loop does.
fn press(app: &mut App, keys: &str) -> Buffer {
    let keys: Vec<_> = keys.split(',').filter(|k| !k.is_empty()).map(|k| shot::key(k).unwrap()).collect();
    shot::render(app, 160, 45, &keys)
}

/// The text of the row the cursor is drawn on (lit with the selection colour), below the
/// header (whose active view tab is lit the same way).
fn lit_row(buf: &Buffer, t: &Theme) -> Option<String> {
    (1..buf.area.height).find_map(|y| {
        let lit = (0..buf.area.width).any(|x| [t.sel, t.needs_sel].contains(&buf[(x, y)].bg) && buf[(x, y)].symbol() != " ");
        lit.then(|| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>())
    })
}

fn screen(buf: &Buffer) -> String {
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>() + "\n")
        .collect()
}

#[test]
fn tab_cycles_the_views_and_shift_tab_goes_back() {
    let mut a = app();
    assert_eq!(a.view, ViewId::Columns);
    press(&mut a, "tab");
    assert_eq!(a.view, ViewId::Timeline);
    press(&mut a, "tab");
    assert_eq!(a.view, ViewId::Finder);
    press(&mut a, "tab");
    assert_eq!(a.view, ViewId::Columns);
    press(&mut a, "backtab");
    assert_eq!(a.view, ViewId::Finder);
    press(&mut a, "shift+tab");
    assert_eq!(a.view, ViewId::Timeline);
}

#[test]
fn slash_opens_finder_where_letters_are_text_and_esc_clears_then_goes_back() {
    let mut a = app();
    press(&mut a, "tab,/");
    assert_eq!(a.view, ViewId::Finder);
    let buf = press(&mut a, "q,u,e,s,x,?,n");
    assert!(!a.quit, "q is query text in Finder");
    assert!(a.overlay.is_none(), "x and ? are query text in Finder");
    assert!(screen(&buf).contains("› quesx?n"), "{}", screen(&buf));
    assert!(!a.prefs.show_ended, "e is query text in Finder");
    press(&mut a, "esc");
    assert_eq!(a.view, ViewId::Finder, "the first Esc clears the query");
    press(&mut a, "esc");
    assert_eq!(a.view, ViewId::Timeline, "the next goes back to where / came from");
    press(&mut a, "esc");
    assert_eq!(a.view, ViewId::Columns);
}

#[test]
fn esc_never_quits_and_ctrl_c_always_does() {
    for view in ViewId::ALL {
        let mut a = app();
        a.view = view;
        press(&mut a, "esc,esc,esc");
        assert!(!a.quit);
        press(&mut a, "f1");
        assert!(matches!(a.overlay, Some(Overlay::Help)));
        press(&mut a, "ctrl+c");
        assert!(a.quit, "Ctrl+C quits in {view:?}, even over the help");
    }
    let mut a = app();
    press(&mut a, "q");
    assert!(a.quit);
}

#[test]
fn ctrl_z_in_the_board_is_ignored_with_a_hint() {
    let mut a = app();
    press(&mut a, "ctrl+z");
    assert!(!a.quit);
    assert!(a.toasts().any(|t| t.text.contains("Ctrl+Z")));
}

#[test]
fn enter_jumps_into_the_selected_session_and_an_elsewhere_one_is_refused() {
    let mut a = app();
    a.cursor = Some(Cursor::Session("a1".into()));
    press(&mut a, "");
    a.handle_key(shot::key("enter").unwrap());
    assert_eq!(a.take_effects(), [Effect::Enter(Enter::Attach { job: "a1b2c3d4".into() })]);
    assert_eq!(a.cursor, Some(Cursor::Session("a1".into())));

    a.cursor = Some(Cursor::Session("w1".into()));
    a.handle_key(shot::key("enter").unwrap());
    assert!(a.take_effects().is_empty());
    assert!(a.toasts().any(|t| t.text.contains("another terminal")));
}

#[test]
fn stop_and_delete_ask_actions_rs_and_confirm() {
    let mut a = app();
    a.cursor = Some(Cursor::Session("w1".into()));
    press(&mut a, "x");
    assert!(a.overlay.is_none(), "an elsewhere session can't be deleted");
    a.cursor = Some(Cursor::Session("a2".into()));
    let buf = press(&mut a, "x");
    assert!(matches!(a.overlay, Some(Overlay::Confirm(_))));
    assert!(screen(&buf).contains("Delete “Upgrade the ORM to v3”?"));
    press(&mut a, "n");
    assert!(a.overlay.is_none());
    a.cursor = Some(Cursor::Session("i1".into()));
    press(&mut a, "s");
    assert!(matches!(a.overlay, Some(Overlay::Confirm(_))), "a running session asks before stopping");
    press(&mut a, "y");
    assert!(a.overlay.is_none());
}

#[test]
fn e_shows_and_hides_ended_sessions() {
    let mut a = app();
    a.cursor = Some(Cursor::Session("a2".into())); // Columns shows one project: api, with a3
    assert!(!screen(&press(&mut a, "")).contains("Rate limiter design"));
    assert!(screen(&press(&mut a, "e")).contains("Rate limiter design"));
    assert!(a.prefs.show_ended);
    assert!(!screen(&press(&mut a, "e")).contains("Rate limiter design"));
}

#[test]
fn the_cursor_stays_on_its_session_through_a_reorder_and_a_view_switch() {
    let t = Theme::default();
    let mut a = app();
    a.cursor = Some(Cursor::Session("d2".into()));
    let before = lit_row(&press(&mut a, ""), &t).unwrap();
    assert!(before.contains("dbt test failures"), "{before}");

    // A refresh in which d2 needs you and everything else changed order.
    let mut snap = sample();
    snap.sessions.reverse();
    snap.sessions.iter_mut().find(|s| s.session_id == "d2").unwrap().state = State::NeedsYou;
    snap.generation += 1;
    a.on_collect(Ok(snap));
    let after = lit_row(&press(&mut a, ""), &t).unwrap();
    assert!(after.contains("dbt test failures"), "{after}");

    for _ in 0..3 {
        let row = lit_row(&press(&mut a, "tab"), &t).unwrap();
        assert!(row.contains("dbt test failures"), "{:?}: {row}", a.view);
    }
    // A move in one view carries to the next.
    press(&mut a, "down,tab");
    let moved = a.cursor.clone();
    assert_ne!(moved, Some(Cursor::Session("d2".into())));
    press(&mut a, "tab");
    assert_eq!(a.cursor, moved);
}

#[test]
fn a_failed_collect_keeps_the_snapshot_and_says_why() {
    let mut a = app();
    a.on_collect(Err("Couldn't read the sessions: disk on fire".into()));
    assert_eq!(a.snapshot.sessions.len(), sample().sessions.len());
    assert!(screen(&press(&mut a, "")).contains("disk on fire"));
}

#[test]
fn every_view_draws_and_takes_keys_at_every_size() {
    let keys = "down,down,pgdn,end,home,up,left,right,enter,x,n,s,esc,f1,esc,?,esc,e,/,a,b,backspace,ctrl+w,ctrl+u,\
                esc,esc,tab,backtab,ctrl+z,c,t,ctrl+k,ctrl+p,ctrl+n,r,e";
    let keys: Vec<_> = keys.split(',').map(|k| shot::key(k).unwrap()).collect();
    let mut sizes: Vec<(u16, u16)> = vec![(1, 1), (10, 3), (29, 7), (30, 8)];
    for w in [40, 41, 64, 80, 109, 110, 160, 240] {
        for h in [12, 13, 24, 45, 70] {
            sizes.push((w, h));
        }
    }
    for (w, h) in sizes {
        for view in ViewId::ALL {
            for snap in [sample(), Snapshot::default()] {
                let mut a = App::offline(snap, "/home/u", "/tmp", Theme::default());
                a.view = view;
                a.cursor = Some(Cursor::Session("a1".into()));
                shot::render(&mut a, w, h, &keys);
            }
        }
    }
}

#[test]
fn board_shot_writes_an_svg() {
    let tmp = TempDir::new();
    let out = tmp.join("shot.svg");
    let data = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sample.json");
    let status = Command::new(env!("CARGO_BIN_EXE_board"))
        .args(["--shot", out.to_str().unwrap(), "--size", "100x30", "--view", "finder", "--data", data, "--keys", "a,u"])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", tmp.path())
        .env("SESSION_BOARD_CONFIG", tmp.join("no-such-config.toml"))
        .env("CLAUDE_CONFIG_DIR", tmp.join("claude"))
        .status()
        .unwrap();
    assert!(status.success());
    let svg = std::fs::read_to_string(&out).unwrap();
    assert!(svg.starts_with("<svg") && svg.trim_end().ends_with("</svg>"));
    // Text is placed word by word.
    assert!(svg.contains(">flaky<"), "the query 'au' keeps “Fix the flaky auth test”");
    assert!(svg.contains(r#"width="900""#), "100 cells of 9px");

    let bad = Command::new(env!("CARGO_BIN_EXE_board")).args(["--shot", "x.svg"]).env_clear().status().unwrap();
    assert_eq!(bad.code(), Some(2));
}
