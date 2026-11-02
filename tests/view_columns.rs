//! Columns on ratatui's `TestBackend` with the sample fixture: the pinned "Needs you" entry, a
//! project, ended sessions, the narrow layouts, the conversation's scroll and `c`.

use board::actions::{Cursor, Enter};
use board::facts::{Session, Snapshot, State};
use board::shot;
use board::tui::app::{App, Effect, ViewId};
use board::tui::theme::Theme;
use ratatui::buffer::Buffer;

fn sample() -> Snapshot {
    let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/sample.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

fn app() -> App {
    let a = App::offline(sample(), "/home/u", "/tmp", Theme::default());
    assert_eq!(a.view, ViewId::Columns, "Columns is the default view");
    a
}

fn press(app: &mut App, w: u16, h: u16, keys: &str) -> Buffer {
    let keys: Vec<_> = keys.split(',').filter(|k| !k.is_empty()).map(|k| shot::key(k).unwrap()).collect();
    shot::render(app, w, h, &keys)
}

fn screen(buf: &Buffer) -> String {
    (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect::<String>() + "\n")
        .collect()
}

fn line(buf: &Buffer, y: u16) -> String {
    (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect()
}

/// Rows (below the header) lit in the focused column's selection colours.
fn lit(buf: &Buffer) -> Vec<String> {
    let t = Theme::default();
    (1..buf.area.height)
        .filter(|&y| (0..buf.area.width).any(|x| [t.sel, t.needs_sel].contains(&buf[(x, y)].bg) && buf[(x, y)].symbol().trim() != ""))
        .map(|y| line(buf, y))
        .collect()
}

fn selected(a: &App) -> Option<String> {
    a.selected().map(str::to_string)
}

#[test]
fn opens_on_needs_you_with_the_newest_selected() {
    let mut a = app();
    let buf = press(&mut a, 160, 45, "");
    let s = screen(&buf);
    assert_eq!(selected(&a).as_deref(), Some("a1"), "the newest session that needs you");
    assert!(s.contains("NEEDS YOU  3 waiting, newest first"), "{s}");
    assert!(s.contains("◆ 3 need you"));
    // Pinned entry, then projects in the board's order (infra, web, api, data).
    let left: String = s.lines().skip(1).map(|l| l.chars().take(22).collect::<String>() + "\n").collect();
    let order: Vec<usize> = ["Needs you", "infra", "web", "api", "data"].iter().map(|p| left.find(&format!(" {p} ")).unwrap()).collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{s}");
    // Under "Needs you" each row names its project.
    assert!(s.contains("api  Reply yes to commit the fix"), "{s}");
    // The conversation: the ask, the reply, the waiting panel; the strip's gauge is labelled.
    assert!(s.contains("WAITING ON YOU  for 5m"), "{s}");
    assert!(s.contains("The auth integration test fails"));
    assert!(s.contains("claude · Opus 5.5"));
    assert!(s.contains("182k") && s.contains("of 1M"));
    assert!(s.contains("~/work/api"), "home shown as ~");
    let rows = lit(&buf);
    assert!(rows[0].contains("Fix the flaky auth test"), "{rows:?}");
    // Enter jumps into it.
    a.handle_key(shot::key("enter").unwrap());
    assert_eq!(a.take_effects(), [Effect::Enter(Enter::Attach { job: "a1b2c3d4".into() })]);
}

#[test]
fn a_tool_call_splits_into_tool_and_command() {
    let mut a = app();
    let s = screen(&press(&mut a, 160, 45, "down"));
    assert_eq!(selected(&a).as_deref(), Some("i3"));
    assert!(s.contains("Approve this tool call?"), "{s}");
    assert!(s.contains(" Bash   $ kubectl -n edge rollout restart deploy/ingress"), "{s}");
}

#[test]
fn a_project_shows_its_sessions_and_enter_on_it_moves_in() {
    let mut a = app();
    // ← to projects, ↓ to infra: its remembered (first) session is selected.
    let buf = press(&mut a, 160, 45, "left,down");
    let s = screen(&buf);
    assert_eq!(selected(&a).as_deref(), Some("i3"));
    assert!(s.contains("INFRA  ~/work/infra"), "{s}");
    assert!(s.contains("Terraform plan for the new queue") && s.contains("Postgres failover drill"));
    // A stopped idle job says so, once: its summary ("Stopped.") would only repeat the word.
    assert!(s.lines().any(|l| l.contains("    stopped ")) && !s.contains("stopped  Stopped."), "{s}");
    assert!(s.contains(" PROJECTS ") && line(&buf, buf.area.height - 1).contains("PROJECTS"));
    // Enter on a project moves into it rather than jumping.
    press(&mut a, 160, 45, "enter");
    assert!(a.take_effects().is_empty());
    assert!(line(&press(&mut a, 160, 45, ""), 44).contains("SESSIONS"));
    // Move within the project, then away and back: the selection is remembered per project.
    press(&mut a, 160, 45, "down,down");
    let here = selected(&a);
    assert_eq!(here.as_deref(), Some("i4"));
    press(&mut a, 160, 45, "left,down,up");
    assert_eq!(selected(&a), here, "back on infra, its session again");
    press(&mut a, 160, 45, "up");
    assert_eq!(selected(&a).as_deref(), Some("a1"), "Needs you remembers its own");
}

#[test]
fn ended_sessions_sit_under_a_divider_when_shown() {
    let mut a = app();
    a.cursor = Some(Cursor::Session("w1".into()));
    let s = screen(&press(&mut a, 160, 45, ""));
    assert!(!s.contains("Bundle size regression") && !s.contains("ENDED ·"), "ended hidden by default");
    let s = screen(&press(&mut a, 160, 45, "e"));
    assert!(s.contains("ENDED · 1"), "{s}");
    assert!(s.find("Landing page copy review").unwrap() < s.find("ENDED · 1").unwrap());
    assert!(s.find("ENDED · 1").unwrap() < s.find("Bundle size regression").unwrap());
    // Down onto it: the conversation says Enter resumes it, unless its folder is gone.
    let s = screen(&press(&mut a, 160, 45, "down"));
    assert_eq!(selected(&a).as_deref(), Some("w3"));
    assert!(s.contains("ended 1d ago"), "{s}");
    // Hiding ended again moves the cursor off it, within the project.
    press(&mut a, 160, 45, "e");
    assert_eq!(selected(&a).as_deref(), Some("w2"));
}

#[test]
fn the_cursor_follows_a_refresh_by_id() {
    let mut a = app();
    a.cursor = Some(Cursor::Session("d2".into()));
    press(&mut a, 160, 45, "");
    let mut snap = sample();
    snap.sessions.reverse();
    snap.sessions.retain(|s| s.session_id != "d1");
    snap.generation += 1;
    a.on_collect(Ok(snap));
    let buf = press(&mut a, 160, 45, "");
    assert_eq!(selected(&a).as_deref(), Some("d2"));
    assert!(lit(&buf)[0].contains("dbt test failures"));
    // Its session gone, the cursor stays in the project.
    let mut snap = sample();
    snap.sessions.retain(|s| s.session_id != "d2");
    a.on_collect(Ok(snap));
    press(&mut a, 160, 45, "");
    assert_eq!(selected(&a).as_deref(), Some("d1"));
}

#[test]
fn narrow_shows_two_columns_then_one() {
    let mut a = app();
    let buf = press(&mut a, 80, 40, "");
    let s = screen(&buf);
    assert!(!s.contains(" PROJECTS "), "sessions and conversation only: {s}");
    assert!(line(&buf, 0).contains("‹"), "a hidden column is hinted in the breadcrumb");
    assert!(s.contains("WAITING ON YOU"));
    let s = screen(&press(&mut a, 80, 40, "left"));
    assert!(s.contains(" PROJECTS ") && s.contains("NEEDS YOU"), "projects and sessions: {s}");
    assert!(!s.contains("WAITING ON YOU"));
    // One column under 64.
    let s = screen(&press(&mut a, 50, 45, "right"));
    assert!(s.contains("Fix the flaky auth") && !s.contains("WAITING ON YOU"), "{s}");
    let s = screen(&press(&mut a, 50, 45, "right"));
    assert!(s.contains("WAITING ON YOU") && !s.contains("waiting, newest"), "{s}");
}

#[test]
fn the_conversation_scrolls_with_its_own_focus() {
    let mut snap = sample();
    let long: String = (1..=60).map(|i| format!("Line {i} of a long reply.\n")).collect();
    let s: &mut Session = snap.sessions.iter_mut().find(|s| s.session_id == "a1").unwrap();
    s.said = Some(long);
    let mut a = App::offline(snap, "/home/u", "/tmp", Theme::default());
    let s = screen(&press(&mut a, 160, 45, ""));
    assert!(s.contains("Line 1 of") && !s.contains("Line 60 of"));
    let s = screen(&press(&mut a, 160, 45, "right,end"));
    assert!(s.contains("Line 60 of") && s.contains("WAITING ON YOU"), "{s}");
    assert!(line(&press(&mut a, 160, 45, ""), 0).contains("conversation"));
    assert_eq!(selected(&a).as_deref(), Some("a1"), "scrolling doesn't move the cursor");
    let s = screen(&press(&mut a, 160, 45, "home"));
    assert!(s.contains("Line 1 of"));
    let s = screen(&press(&mut a, 160, 45, "pgdn,pgdn"));
    assert!(!s.contains("Fix the flaky auth test\n") || s.contains("Line 40"), "paged down");
    // ← gives the keys back to the sessions; ↓ moves to the next and the scroll resets.
    let s = screen(&press(&mut a, 160, 45, "left,down"));
    assert_eq!(selected(&a).as_deref(), Some("i3"));
    assert!(s.contains("Approve: kubectl rollout restart\n") || s.contains("Roll the ingress pods"));
}

#[test]
fn c_toggles_the_metadata_strip() {
    let mut a = app();
    assert!(a.prefs.show_columns);
    assert!(screen(&press(&mut a, 160, 45, "")).contains("MODEL Opus 5.5"));
    let s = screen(&press(&mut a, 160, 45, "c"));
    assert!(!a.prefs.show_columns);
    assert!(!s.contains("MODEL ") && !s.contains("of 1M"), "{s}");
    assert!(screen(&press(&mut a, 160, 45, "c")).contains("MODEL Opus 5.5"));
}

#[test]
fn an_empty_needs_you_says_so() {
    let mut snap = sample();
    for s in &mut snap.sessions {
        if s.state == State::NeedsYou {
            s.state = State::Idle;
        }
    }
    let mut a = App::offline(snap, "/home/u", "/tmp", Theme::default());
    let s = screen(&press(&mut a, 160, 45, ""));
    assert!(!s.contains("need you"));
    assert!(s.contains("INFRA"), "with nothing waiting it opens on the first project: {s}");
    let s = screen(&press(&mut a, 160, 45, "left,up"));
    assert!(s.contains("✓ Nothing needs you"), "{s}");
    let empty = App::offline(Snapshot::default(), "/home/u", "/tmp", Theme::default());
    let mut empty = empty;
    press(&mut empty, 160, 45, "down,right,enter,left,c");
}
