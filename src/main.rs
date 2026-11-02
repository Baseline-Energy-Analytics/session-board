//! `board`: the command line. With no options, the terminal app; otherwise the scripting
//! outputs (`--json`, `--list`, `--config`, `--dump`) and `--shot`.

use board::config::{self, Config};
use board::facts::{self, Sources};
use board::{report, shot, tui};
use std::process::ExitCode;

const HELP: &str = "\
usage: board [--list | --json | --config | --dump]
       board --shot FILE.svg --size WxH [--view V] [--data JSON] [--select ID] [--keys K,…]
                    [--home DIR]

  Every Claude Code session on this machine: what needs you, what is running,
  what is idle and what has ended, by project. Enter jumps into a session;
  Ctrl+Z inside it (or ← then Esc) comes back here.

  Views (Tab / Shift+Tab cycles them): Columns, Timeline, Finder.

  Keys:  Enter          jump in (an ended session is resumed in the background first)
         n              new background session (folder, name, first prompt), then jump in
                        (in the form: Tab or ↑↓ between fields, Enter starts,
                        Esc cancels; folders don't tab-complete yet)
         s              stop a background session; Enter opens it again
         x              delete a background session (asks first; can't be undone)
         e              show or hide ended sessions (hidden at first; remembered)
         /              search, in Finder (ended sessions too)
         r              refresh;  q or Ctrl+C  quit
         ← →            Columns: between columns;  c  its metadata strip (remembered)
         f              Columns: every file in the Files card, or the latest few (remembered)
         t              Timeline: 24 hours / 7 days
         Ctrl+K         Finder: actions menu, New session… too (letters are search
                        text there, so it also has Ctrl+S stop, Ctrl+X delete,
                        Ctrl+E ended)
         Esc            close a dialog, clear the search, or go back; never quits
         F1             help (? too, outside Finder)

  --list    print the board as text and exit
  --json    print every session as JSON and exit: an array of objects with
            session_id, job, title, state, stopped, was, project, cwd, last
            (epoch seconds), model, context, output. No transcript text.
  --config  print the settings in effect (workspaces, scratch folder, …) and
            where each came from: SESSION_BOARD_* variables, then
            ~/.config/session-board/config.toml, then defaults
  --dump    print the full snapshot as JSON, transcript excerpts included
            (for fixtures and debugging; it can hold private text)
  --shot    draw one frame as an SVG: --view columns|timeline|finder, --data a
            --dump file (else this machine's sessions), --select a session id,
            --keys to press first (down,enter,ctrl+k,n,…; they can open the
            help, a confirm, the new-session form or the menu), --home the
            folder shown as ~

  States (the \"state\" field in --json, in this order of urgency):
    needs you  a background session waiting on you (a question, an approval)
    running    a background session working on a turn
    elsewhere  open in another terminal; shown as \"in another terminal\"
    idle       a background session with nothing to do; \"stopped\" if stopped
               with s (\"stopped\": true), and Enter opens it again
    stale      a background job whose state says it needs you or is working,
               but Claude Code has no process for it; shown as \"not running\".
               \"was\" holds the state it still claims.
    ended      a saved conversation only; Enter resumes it";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    if has("-h") || has("--help") {
        println!("{HELP}");
        return ExitCode::SUCCESS;
    }
    let shot = if has("--shot") {
        match shot::parse(&args) {
            Ok(s) => Some(s),
            Err(e) => {
                eprintln!("board: {e}; see board --help");
                return ExitCode::from(2);
            }
        }
    } else {
        None
    };
    const KNOWN: [&str; 4] = ["--list", "--json", "--config", "--dump"];
    if let Some(bad) = args.iter().find(|a| !KNOWN.contains(&a.as_str())).filter(|_| shot.is_none()) {
        eprintln!("board: unknown option {bad}; see board --help");
        return ExitCode::from(2);
    }
    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    if has("--config") {
        println!("{}", config::describe(&config));
        return ExitCode::SUCCESS;
    }
    if let Some(shot) = shot {
        return match shot::run(&shot, &config) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("board: {e}");
                ExitCode::FAILURE
            }
        };
    }
    let wanted = ["--list", "--json", "--dump"].into_iter().find(|f| has(f));
    let Some(mode) = wanted else {
        return match tui::run(&config) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("board: {e}");
                ExitCode::FAILURE
            }
        };
    };
    let mut sources = Sources::from_config(&config);
    sources.git = mode == "--dump"; // --list and --json don't show it
    let snapshot = facts::collect(&sources, 1);
    match mode {
        "--list" => print!("{}", report::list(&snapshot.sessions, facts::now(), &config.home, &config.scratch)),
        "--json" => println!("{}", report::json(&snapshot.sessions)),
        _ => println!("{}", report::dump(&snapshot)),
    }
    ExitCode::SUCCESS
}
