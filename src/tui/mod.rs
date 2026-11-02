//! The terminal app. `run` owns the terminal and the main loop; [`app::App`] is everything
//! else and runs the same on a `TestBackend`.
//!
//! All input is read here, on the main thread, with `event::poll`: never from an input thread,
//! which would go on reading stdin while `claude attach` owns the terminal and steal its keys.
//! The collect thread never touches the terminal.

pub mod app;
pub mod overlays;
pub mod terminal;
pub mod theme;
pub mod views;
pub mod widgets;
pub mod worker;

use crate::actions::{Cli, Enter, SystemRunner};
use crate::config::Config;
use crate::facts::Sources;
use crate::store;
use app::{App, Done, Effect, Settings, Task, Tone};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event};
use std::io;
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};
use theme::Theme;
use worker::Worker;

/// How often the loop wakes to look for snapshots and expired toasts, and the animation tick.
/// A frame is drawn only when something changed.
const TICK: Duration = Duration::from_millis(250);

/// The thread a stop or delete runs on. Like the collect thread it never touches the terminal,
/// and a panic on it leaves the terminal alone.
pub const CLI_THREAD: &str = "board-cli";

/// `board` with no options: the board, until `q` or Ctrl+C.
pub fn run(config: &Config) -> io::Result<()> {
    let mut app = App::new(Settings::from_config(config), store::load_prefs(&config.prefs_path()), Theme::detect());
    let worker = Worker::start(Sources::from_config(config), Duration::from_secs_f64(config.poll));
    let mut terminal = terminal::init()?;
    let result = main_loop(&mut app, &mut terminal, &worker);
    terminal::restore();
    result
}

fn main_loop(app: &mut App, terminal: &mut DefaultTerminal, worker: &Worker) -> io::Result<()> {
    let mut dirty = true;
    let mut last_tick = Instant::now();
    let (done_tx, done_rx): (Sender<Done>, Receiver<Done>) = mpsc::channel();
    while !app.quit {
        if let Some(result) = worker.try_recv() {
            app.on_collect(result);
            dirty = true;
        }
        while let Ok(done) = done_rx.try_recv() {
            app.on_done(done);
            dirty = true;
        }
        dirty |= app.expire_toasts();
        if app.animating() && last_tick.elapsed() >= TICK {
            app.tick += 1;
            last_tick = Instant::now();
            dirty = true;
        }
        if dirty {
            terminal.draw(|f| app.draw(f))?;
            dirty = false;
        }
        if event::poll(TICK)? {
            match event::read()? {
                Event::Key(key) => app.handle_key(key),
                Event::Resize(..) => {}
                _ => continue,
            }
            dirty = true;
        }
        // Effects can bring more (a started session is attached to), so until there are none.
        loop {
            let effects = app.take_effects();
            if effects.is_empty() {
                break;
            }
            for effect in effects {
                match effect {
                    Effect::Refresh => worker.refresh(),
                    Effect::Enter(how) => jump(app, terminal, worker, how)?,
                    Effect::Run(task) => run_task(app, terminal, task, &done_tx)?,
                }
                dirty = true;
            }
        }
    }
    Ok(())
}

/// Run a claude command for an action. Stop and delete go to their own thread, so a slow CLI
/// never freezes the board; their result comes back through `done`. Starting a session runs
/// here, after a frame showing its toast: it is attached to straight after, and `claude --bg`
/// shares the board's stdin, which no thread may read while the board reads keys.
fn run_task(app: &mut App, terminal: &mut DefaultTerminal, task: Task, done: &Sender<Done>) -> io::Result<()> {
    let cli = Cli { claude: app.settings.claude.clone(), home: app.settings.home.clone(), runner: SystemRunner };
    let (deleted, days, now) = (app.settings.deleted_path.clone(), app.settings.days, crate::facts::now());
    if let Task::New(_) = task {
        terminal.draw(|f| app.draw(f))?;
        let d = task.perform(&cli, deleted.as_deref(), days, now);
        app.on_done(d);
        return Ok(());
    }
    let tx = done.clone();
    let spawned = std::thread::Builder::new().name(CLI_THREAD.into()).spawn(move || {
        let _ = tx.send(task.perform(&cli, deleted.as_deref(), days, now));
    });
    if let Err(e) = spawned {
        app.toast(Tone::Error, format!("Couldn't run claude: {e}"));
    }
    Ok(())
}

/// Jump into a session: resume it in the background first if it has ended, then hand the
/// terminal to `claude attach` until it exits (Ctrl+Z or ← then Esc inside it, both exit 0).
/// The cursor stays where it was, since it's keyed by session id.
fn jump(app: &mut App, terminal: &mut DefaultTerminal, worker: &Worker, how: Enter) -> io::Result<()> {
    let cli = Cli { claude: app.settings.claude.clone(), home: app.settings.home.clone(), runner: SystemRunner };
    let job = match how {
        Enter::Attach { job } => job,
        Enter::ResumeThenAttach { session_id, cwd } => {
            app.toast(Tone::Info, "Resuming the conversation in the background…");
            terminal.draw(|f| app.draw(f))?;
            match cli.resume_in_background(&session_id, &cwd) {
                Ok(job) => job,
                Err(why) => {
                    app.toast(Tone::Error, why.0);
                    return Ok(());
                }
            }
        }
    };
    let (argv, cwd) = cli.attach_argv(&job);
    worker.pause();
    let status = terminal::suspend_for(terminal, || Command::new(&argv[0]).args(&argv[1..]).current_dir(&cwd).status());
    worker.resume();
    match status? {
        Ok(s) if s.success() => {}
        Ok(s) => {
            let how = s.code().map_or_else(|| "was killed by a signal".to_string(), |c| format!("exited with code {c}"));
            app.toast(Tone::Error, format!("claude attach {job} {how}"));
        }
        Err(e) => app.toast(Tone::Error, format!("Couldn't run {}: {e}", argv.join(" "))),
    }
    Ok(())
}
