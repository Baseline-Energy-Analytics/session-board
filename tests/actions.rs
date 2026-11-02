//! Twins of tests/test_actions.py, with a fake runner in place of the claude CLI, plus the
//! cursor-after-delete and default-folder rules that lived in app.py.

use board::actions::{self, Cli, Cursor, Enter, NewSession, Refused, Runner};
use board::facts::{Session, State};
use board::proc::{Output, RunError};
use std::cell::RefCell;

mod common;
use common::TempDir;

const HOME: &str = "/home/u";

fn session(state: State) -> Session {
    Session::new("abcdef12-3456", "t", "/", "/", state, 0.0)
}

fn with_job(state: State, job: &str) -> Session {
    Session { job: Some(job.into()), ..session(state) }
}

fn refused<T: std::fmt::Debug>(r: Result<T, Refused>, words: &str) {
    let e = r.expect_err("should be refused");
    assert!(e.0.contains(words), "{:?} doesn't say {words:?}", e.0);
}

/// Records each call and answers with a canned output.
struct Fake {
    out: Output,
    calls: RefCell<Vec<(Vec<String>, String)>>,
}

impl Runner for &Fake {
    fn run(&self, argv: &[String], cwd: &str) -> Result<Output, RunError> {
        self.calls.borrow_mut().push((argv.to_vec(), cwd.to_string()));
        Ok(self.out.clone())
    }
}

fn fake(stdout: &str, code: i32, stderr: &str) -> Fake {
    Fake {
        out: Output { code: Some(code), stdout: stdout.into(), stderr: stderr.into() },
        calls: RefCell::new(Vec::new()),
    }
}

fn cli(f: &Fake) -> Cli<&Fake> {
    Cli { claude: vec!["claude".into()], home: HOME.into(), runner: f }
}

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

#[test]
fn background_session_attaches() {
    assert_eq!(actions::plan_enter(&with_job(State::Idle, "abcdef12")), Ok(Enter::Attach { job: "abcdef12".into() }));
}

#[test]
fn ended_session_resumes_in_its_folder() {
    let tmp = TempDir::new();
    let s = Session { cwd: tmp.str(), ..session(State::Ended) };
    assert_eq!(
        actions::plan_enter(&s),
        Ok(Enter::ResumeThenAttach { session_id: "abcdef12-3456".into(), cwd: tmp.str() })
    );
}

#[test]
fn elsewhere_is_refused() {
    refused(actions::plan_enter(&session(State::Elsewhere)), "another terminal");
}

#[test]
fn ended_session_in_a_deleted_folder_is_refused() {
    let s = Session { cwd: "/nonexistent/folder".into(), ..session(State::Ended) };
    refused(actions::plan_enter(&s), "no longer exists");
}

#[test]
fn resume_parses_the_short_id() {
    let f = fake("Starting background service…\nbackgrounded · 9e3c0f69 (idle — send a prompt to start)\n", 0, "");
    assert_eq!(cli(&f).resume_in_background("9e3c0f69-cdfd", "/opt"), Ok("9e3c0f69".into()));
    assert_eq!(*f.calls.borrow(), [(argv(&["claude", "--bg", "--resume", "9e3c0f69-cdfd"]), "/opt".into())]);
}

#[test]
fn resume_falls_back_to_the_id_prefix() {
    let f = fake("something new\n", 0, "");
    assert_eq!(cli(&f).resume_in_background("9e3c0f69-cdfd", "/opt"), Ok("9e3c0f69".into()));
}

#[test]
fn resume_failure_is_refused_with_the_reason() {
    let f = fake("", 1, "No conversation found");
    refused(cli(&f).resume_in_background("x", "/opt"), "No conversation found");
}

// --- new, stop, delete ---------------------------------------------------------------------------

#[test]
fn plan_new_expands_the_folder_and_tidies_the_name() {
    let tmp = TempDir::new();
    let req = actions::plan_new(&format!(" {} ", tmp.str()), "  fix   the  cert ", " go \n", HOME).unwrap();
    assert_eq!(req, NewSession { cwd: tmp.str(), name: "fix the cert".into(), prompt: "go".into() });
}

#[test]
fn plan_new_expands_tilde() {
    let tmp = TempDir::new();
    assert_eq!(actions::plan_new("~", "x", "", &tmp.str()).unwrap().cwd, tmp.str());
}

#[test]
fn plan_new_refuses_a_missing_folder() {
    refused(actions::plan_new("/nonexistent/folder", "x", "", HOME), "no folder /nonexistent/folder");
    refused(actions::plan_new("  ", "x", "", HOME), "which folder");
}

#[test]
fn plan_new_needs_a_name_or_a_prompt() {
    let tmp = TempDir::new();
    refused(actions::plan_new(&tmp.str(), " ", "", HOME), "name or a first prompt");
    assert_eq!(actions::plan_new(&tmp.str(), "", "do it", HOME).unwrap().prompt, "do it");
}

#[test]
fn new_argv_keeps_a_dash_prompt_from_being_read_as_an_option() {
    let req = |name: &str, prompt: &str| NewSession { cwd: "/opt".into(), name: name.into(), prompt: prompt.into() };
    assert_eq!(actions::new_argv(&req("n", "-x do it")), argv(&["--bg", "--name", "n", "--", "-x do it"]));
    assert_eq!(actions::new_argv(&req("n", "")), argv(&["--bg", "--name", "n"]));
    assert_eq!(actions::new_argv(&req("", "p")), argv(&["--bg", "--", "p"]));
}

#[test]
fn start_parses_the_short_id_and_runs_in_the_folder() {
    let f = fake(
        "backgrounded · 83297fbd · sb-test (idle — send a prompt to start)\n  claude attach 83297fbd    open in this terminal\n",
        0,
        "",
    );
    let req = NewSession { cwd: "/srv".into(), name: "sb-test".into(), prompt: String::new() };
    assert_eq!(cli(&f).start_in_background(&req), Ok("83297fbd".into()));
    assert_eq!(*f.calls.borrow(), [(argv(&["claude", "--bg", "--name", "sb-test"]), "/srv".into())]);
}

#[test]
fn start_without_an_id_is_refused() {
    let f = fake("something new\n", 0, "");
    let req = NewSession { cwd: "/srv".into(), name: "x".into(), prompt: String::new() };
    refused(cli(&f).start_in_background(&req), "couldn't read its id");
}

#[test]
fn stop_and_delete_run_the_cli() {
    let f = fake("stopped 83297fbd\n", 0, "");
    cli(&f).stop("83297fbd").unwrap();
    cli(&f).delete("83297fbd").unwrap();
    let calls: Vec<Vec<String>> = f.calls.borrow().iter().map(|c| c.0.clone()).collect();
    assert_eq!(calls, [argv(&["claude", "stop", "83297fbd"]), argv(&["claude", "rm", "83297fbd"])]);
    assert!(f.calls.borrow().iter().all(|c| c.1 == HOME));
}

#[test]
fn delete_failure_carries_the_cli_message() {
    let f = fake("No job matching '83297fbd'\n", 1, "");
    refused(cli(&f).delete("83297fbd"), "No job matching");
}

#[test]
fn a_silent_failure_names_the_exit_code() {
    let f = fake("", 3, "");
    refused(cli(&f).stop("j"), "Couldn't stop the session: exit code 3");
}

#[test]
fn a_command_that_cant_start_is_refused() {
    let c = Cli { claude: vec!["/nonexistent/claude".into()], home: HOME.into(), runner: actions::SystemRunner };
    refused(c.stop("j"), "Couldn't stop the session");
}

#[test]
fn a_multi_word_claude_comes_first() {
    let f = fake("", 0, "");
    let c = Cli { claude: argv(&["ssh", "box", "claude"]), home: HOME.into(), runner: &f };
    c.stop("j").unwrap();
    assert_eq!(f.calls.borrow()[0].0, argv(&["ssh", "box", "claude", "stop", "j"]));
    assert_eq!(c.attach_argv("j"), (argv(&["ssh", "box", "claude", "attach", "j"]), HOME.into()));
}

#[test]
fn plan_stop_refuses() {
    refused(actions::plan_stop(&session(State::Elsewhere)), "stop it there");
    refused(actions::plan_stop(&session(State::Ended)), "nothing to stop");
    let stopped = Session { stopped: true, ..with_job(State::Idle, "j") };
    refused(actions::plan_stop(&stopped), "already stopped");
}

#[test]
fn plan_stop_and_delete_return_the_job() {
    let s = with_job(State::Running, "j");
    assert_eq!(actions::plan_stop(&s), Ok("j".into()));
    assert_eq!(actions::plan_delete(&s, 30), Ok("j".into()));
    let stopped = Session { stopped: true, ..with_job(State::Idle, "j") };
    assert_eq!(actions::plan_delete(&stopped, 30), Ok("j".into()));
}

#[test]
fn plan_delete_refuses() {
    refused(actions::plan_delete(&session(State::Elsewhere), 30), "another terminal");
    refused(actions::plan_delete(&session(State::Ended), 30), "saved conversation");
    refused(actions::plan_delete(&session(State::Ended), 14), "after 14 days");
}

#[test]
fn confirm_texts_mention_a_running_session() {
    let running = with_job(State::Running, "j");
    assert!(actions::delete_confirm(&running, 30).contains("working right now; deleting stops it"));
    assert!(!actions::delete_confirm(&with_job(State::Idle, "j"), 30).contains("working right now"));
    assert!(actions::stop_confirm(&running).is_some());
    assert!(actions::stop_confirm(&with_job(State::Idle, "j")).is_none()); // stops at once
}

// --- cursor after a delete; the new-session form's folder ---------------------------------------

#[test]
fn after_delete_goes_next_then_previous_then_the_project() {
    let rows = vec![
        ("/p1".to_string(), vec!["a".to_string(), "b".to_string(), "c".to_string()]),
        ("/p2".to_string(), vec!["d".to_string()]),
    ];
    assert_eq!(actions::after_delete(&rows, "a"), Some(Cursor::Session("b".into())));
    assert_eq!(actions::after_delete(&rows, "c"), Some(Cursor::Session("b".into())));
    assert_eq!(actions::after_delete(&rows, "d"), Some(Cursor::Project("/p2".into())));
    assert_eq!(actions::after_delete(&rows, "zzz"), None);
}

#[test]
fn default_folder_prefers_the_session_then_its_project_then_home() {
    let tmp = TempDir::new();
    let proj = tmp.join("proj");
    let scratch = tmp.join("scratch");
    let old = scratch.join("old");
    let recent = scratch.join("recent");
    for d in [&proj, &old, &recent] {
        std::fs::create_dir_all(d).unwrap();
    }
    let p = |x: &std::path::Path| x.to_string_lossy().into_owned();
    let mut gone = Session::new("gone", "g", "/nonexistent/x", &p(&proj), State::Ended, 5.0);
    let here = Session::new("here", "h", &p(&proj), &p(&proj), State::Idle, 4.0);
    let s_old = Session::new("so", "o", &p(&old), &p(&scratch), State::Ended, 1.0);
    let s_new = Session::new("sn", "n", &p(&recent), &p(&scratch), State::Ended, 2.0);
    let sessions = vec![gone.clone(), here, s_old, s_new];
    let folder = |c: Option<Cursor>| actions::default_folder(c.as_ref(), &sessions, &p(&scratch), HOME);
    assert_eq!(folder(Some(Cursor::Session("here".into()))), p(&proj));
    assert_eq!(folder(Some(Cursor::Session("gone".into()))), p(&proj)); // its folder is gone: the project
    assert_eq!(folder(Some(Cursor::Project(p(&scratch)))), p(&recent)); // scratch: its latest session's
    assert_eq!(folder(Some(Cursor::Project("/nonexistent".into()))), HOME);
    assert_eq!(folder(None), HOME);
    gone.project = "/nonexistent/p".into();
    let sessions = vec![gone];
    assert_eq!(actions::default_folder(Some(&Cursor::Session("gone".into())), &sessions, "/tmp", HOME), HOME);
}
