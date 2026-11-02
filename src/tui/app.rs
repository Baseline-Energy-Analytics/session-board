//! The board app: the snapshot, the three views, overlays and toasts; key routing; and every
//! [`Action`] executed in one place ([`App::run`]).
//!
//! The app does no I/O of its own except saving prefs. Anything that needs the terminal or the
//! collect thread (jumping into a session, refreshing) becomes an [`Effect`] that the run loop
//! in `tui/mod.rs` carries out, so the whole app runs on a `TestBackend` in tests and shots.
//!
//! Keys go, in order: Ctrl+C (quit, always) → an open overlay (takes everything else) → the
//! app's own keys (Ctrl+Z, F1, Tab, Shift+Tab) → the active view → the global table for keys the
//! view didn't use ([`App::global_key`]).

use super::overlays::{Confirm, NewForm, Overlay, Reply};
use super::theme::Theme;
use super::views;
use super::widgets;
use crate::actions::{self, Cli, Cursor, Enter, NewSession, Refused, Runner};
use crate::config::Config;
use crate::facts::{Session, Snapshot, State, group};
use crate::store::{self, Prefs};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Color;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

// --- views ---------------------------------------------------------------------------------------

/// The three views, in Tab order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ViewId {
    Columns,
    Timeline,
    Finder,
}

impl ViewId {
    pub const ALL: [ViewId; 3] = [ViewId::Columns, ViewId::Timeline, ViewId::Finder];

    fn index(self) -> usize {
        self as usize
    }

    pub fn next(self) -> ViewId {
        ViewId::ALL[(self.index() + 1) % 3]
    }

    pub fn prev(self) -> ViewId {
        ViewId::ALL[(self.index() + 2) % 3]
    }

    /// For people: `Columns`.
    pub fn title(self) -> &'static str {
        match self {
            ViewId::Columns => "Columns",
            ViewId::Timeline => "Timeline",
            ViewId::Finder => "Finder",
        }
    }

    /// For the command line: `columns`.
    pub fn parse(name: &str) -> Option<ViewId> {
        ViewId::ALL.into_iter().find(|v| v.title().eq_ignore_ascii_case(name))
    }
}

/// What a view gets to draw and decide with. Built fresh for every draw and key.
pub struct Ctx<'a> {
    pub snap: &'a Snapshot,
    pub theme: &'a Theme,
    /// The time to measure ages against, epoch seconds (the snapshot's time in shots).
    pub now: f64,
    /// Your home folder, for `~` in paths ([`widgets::short_path`]).
    pub home: &'a str,
    /// The scratch folder, for the "scratch" project ([`widgets::project_name`]).
    pub scratch: &'a str,
    /// How long Claude Code keeps transcripts; for refusal and confirm text.
    pub days: u32,
    /// `show_ended` and `show_columns`.
    pub prefs: Prefs,
    /// Animation frame, one per 250 ms while [`View::animating`] says so: for spinners.
    pub tick: u64,
    /// Whether the first snapshot has arrived; before it, `snap` is empty.
    pub loaded: bool,
    /// The claude command as configured, split into words (`["ssh", "box", "claude"]`): for
    /// showing the commands the board runs.
    pub claude: &'a [String],
}

impl<'a> Ctx<'a> {
    /// The sessions the board shows: all, less ended ones unless `e` turned them on. Sorted by
    /// state, then most recent first.
    pub fn visible(&self) -> impl Iterator<Item = &'a Session> + use<'a> {
        let show_ended = self.prefs.show_ended;
        self.snap.sessions.iter().filter(move |s| show_ended || s.state != State::Ended)
    }

    /// The claude command as one string, for showing: `claude`, `ssh box claude`.
    pub fn claude_cmd(&self) -> String {
        self.claude.join(" ")
    }
}

/// A view: Columns, Timeline or Finder. The struct implementing it holds the view's own state
/// (focus, scroll, query…); what all views share is the cursor, passed in. See
/// `src/tui/views/README.md`.
pub trait View {
    /// Draw into `area` (the whole screen, at least [`MIN_WIDTH`] x [`MIN_HEIGHT`]). Resolve
    /// `cursor` to a row here, from the snapshot as it is now, and fix it up if its session has
    /// gone (see [`views::resolve`]).
    fn draw(&mut self, frame: &mut Frame, area: Rect, cx: &Ctx, cursor: &mut Option<Cursor>);

    /// A key the app didn't take. `None`: not used here, so the global table gets it.
    /// `Some(Action::Handled)`: used (a cursor move). Any other action runs in the app.
    fn handle_key(&mut self, key: KeyEvent, cx: &Ctx, cursor: &mut Option<Cursor>) -> Option<Action>;

    /// True while printable keys are text (Finder's query): the global letter keys (`q`, `/`,
    /// `e`, `n`, `s`, `x`, `r`, `?`) then stay off even for keys the view returns `None` for.
    fn captures_text(&self) -> bool {
        false
    }

    /// The view was switched to. `search`: by `/`, so put the cursor in the query.
    fn shown(&mut self, _search: bool) {}

    /// True while something on screen moves (a running session's spinner), so the app ticks
    /// every 250 ms; otherwise it redraws only on keys, snapshots and toasts.
    fn animating(&self, cx: &Ctx) -> bool {
        cx.visible().any(|s| s.state == State::Running)
    }
}

/// Below this the app draws a "make the window bigger" note instead of the view.
pub const MIN_WIDTH: u16 = 30;
pub const MIN_HEIGHT: u16 = 8;

// --- actions -------------------------------------------------------------------------------------

/// Which view to switch to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Switch {
    Next,
    Prev,
    To(ViewId),
}

/// Something to do, from a view, an overlay or a global key. Carries session ids, never
/// positions. Executed by [`App::run`].
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// The view used the key itself; nothing more to do.
    Handled,
    /// Enter: attach, resuming an ended session in the background first.
    Jump(String),
    /// `s`: stop a background session (asks first if it's working).
    Stop(String),
    /// `x`: delete a background session (always asks).
    Delete(String),
    /// After the confirm; views send [`Action::Stop`].
    StopConfirmed(String),
    /// After the confirm; views send [`Action::Delete`].
    DeleteConfirmed(String),
    /// `n`: the new-session form.
    New,
    /// The form's Enter, checked: start it in the background and jump in.
    StartNew(NewSession),
    /// Ctrl+K in Finder: the action menu for the selected session.
    Menu,
    /// `e`: show or hide ended sessions (remembered).
    ToggleEnded,
    /// `c` in Columns: its metadata strip (remembered).
    ToggleColumns,
    /// `f` in Columns: every file in the Files card, or the latest few (remembered).
    ToggleFiles,
    SwitchView(Switch),
    /// Esc with nothing else to do: the previous view.
    Back,
    /// `/`: Finder, cursor in its query.
    OpenFinderSearch,
    Help,
    Refresh,
    /// Tell the user something (a refusal a view found itself, say).
    Toast(String),
    Quit,
}

/// What the run loop does for the app: things that need the terminal, the collect thread or
/// the claude CLI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    Refresh,
    /// Hand the terminal to `claude attach` (resuming first if needed), then take it back.
    Enter(Enter),
    /// Run a claude command ([`Task::perform`]) and hand its [`Done`] to [`App::on_done`].
    Run(Task),
}

/// A claude command the board runs for an action. The run loop does it off the main thread
/// where it can (stop, delete) and hands the result back as a [`Done`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Task {
    Stop { title: String, job: String },
    /// `after` is where the cursor goes once the row is gone.
    Delete { session_id: String, root: Option<String>, title: String, job: String, after: Option<Cursor> },
    New(NewSession),
}

/// How a [`Task`] went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Done {
    Stopped { title: String, result: Result<(), Refused> },
    Deleted { title: String, after: Option<Cursor>, result: Result<(), Refused> },
    /// The new session's job id, to attach to.
    Started { label: String, result: Result<String, Refused> },
}

impl Task {
    /// Run it. A delete also forgets the conversation in `deleted` (`store::forget`), since
    /// `claude rm` keeps the transcript; failing to write that doesn't fail the delete.
    pub fn perform<R: Runner>(self, cli: &Cli<R>, deleted: Option<&Path>, days: u32, now: f64) -> Done {
        match self {
            Task::Stop { title, job } => Done::Stopped { title, result: cli.stop(&job) },
            Task::Delete { session_id, root, title, job, after } => {
                let result = cli.delete(&job);
                if result.is_ok()
                    && let Some(path) = deleted
                {
                    // The job is gone either way; at worst its conversation shows again as ended.
                    let _ = store::forget(&session_id, root.as_deref(), path, days, now);
                }
                Done::Deleted { title, after, result }
            }
            Task::New(req) => {
                let label = new_label(&req);
                Done::Started { label, result: cli.start_in_background(&req) }
            }
        }
    }
}

/// What to call a session being started: its name, else the start of its prompt.
fn new_label(req: &NewSession) -> String {
    if req.name.is_empty() { widgets::trunc(&req.prompt, 50) } else { req.name.clone() }
}

/// A CLI error for a toast: on one line and short.
fn short_error(why: &Refused) -> String {
    widgets::trunc(&crate::mask::flat(&why.0), 240)
}

/// How long the cursor waits for a newly started session to show up.
const PENDING_GRACE: f64 = 30.0;

// --- toasts --------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Info,
    Ok,
    Warn,
    Error,
}

#[derive(Clone, Debug)]
pub struct Toast {
    pub text: String,
    pub tone: Tone,
    until: Instant,
}

/// The most toasts shown at once; older ones go.
const TOASTS: usize = 3;

// --- the app -------------------------------------------------------------------------------------

/// What the app needs from the config.
#[derive(Clone, Debug)]
pub struct Settings {
    pub home: String,
    pub scratch: String,
    pub days: u32,
    /// The claude CLI, split into words.
    pub claude: Vec<String>,
    /// Where `e` and `c` are remembered; `None` in tests and shots, which never write.
    pub prefs_path: Option<PathBuf>,
    /// Where deleted conversations are remembered (`deleted.json`); `None` in tests and shots.
    pub deleted_path: Option<PathBuf>,
}

impl Settings {
    pub fn from_config(c: &Config) -> Settings {
        Settings {
            home: c.home.clone(),
            scratch: c.scratch.clone(),
            days: c.days,
            claude: c.claude_argv(),
            prefs_path: Some(c.prefs_path()),
            deleted_path: Some(c.deleted_path()),
        }
    }
}

pub struct App {
    pub settings: Settings,
    pub theme: Theme,
    pub snapshot: Snapshot,
    /// Whether a snapshot has arrived yet.
    pub loaded: bool,
    pub prefs: Prefs,
    pub view: ViewId,
    /// Views switched away from, for Esc.
    history: Vec<ViewId>,
    views: [Box<dyn View>; 3],
    /// What the cursor rests on, shared by the views.
    pub cursor: Option<Cursor>,
    pub overlay: Option<Overlay>,
    toasts: VecDeque<Toast>,
    /// Why the last collect failed, until one succeeds.
    pub notice: Option<String>,
    pub tick: u64,
    /// A fixed "now" for shots and tests; else the clock.
    pub clock: Option<f64>,
    effects: Vec<Effect>,
    /// A session just started from the form, and when (epoch seconds): the cursor goes to its
    /// row when a snapshot first shows it, within [`PENDING_GRACE`].
    pub pending: Option<(String, f64)>,
    pub quit: bool,
}

/// Builds a [`Ctx`] from an app's fields, borrowing them one by one so a view (another field)
/// can be borrowed mutably alongside.
macro_rules! cx {
    ($app:expr) => {
        Ctx {
            snap: &$app.snapshot,
            theme: &$app.theme,
            now: $app.clock.unwrap_or_else(crate::facts::now),
            home: &$app.settings.home,
            scratch: &$app.settings.scratch,
            days: $app.settings.days,
            prefs: $app.prefs,
            tick: $app.tick,
            loaded: $app.loaded,
            claude: &$app.settings.claude,
        }
    };
}

impl App {
    pub fn new(settings: Settings, prefs: Prefs, theme: Theme) -> App {
        App {
            settings,
            theme,
            snapshot: Snapshot::default(),
            loaded: false,
            prefs,
            view: ViewId::Columns,
            history: Vec::new(),
            views: views::all(),
            cursor: None,
            overlay: None,
            toasts: VecDeque::new(),
            notice: None,
            tick: 0,
            clock: None,
            effects: Vec::new(),
            pending: None,
            quit: false,
        }
    }

    /// An app showing `snapshot` at its own time, writing nothing: for shots and tests.
    pub fn offline(snapshot: Snapshot, home: &str, scratch: &str, theme: Theme) -> App {
        let settings = Settings {
            home: home.into(),
            scratch: scratch.into(),
            days: 30,
            claude: vec!["claude".into()],
            prefs_path: None,
            deleted_path: None,
        };
        let mut app = App::new(settings, Prefs::default(), theme);
        app.clock = Some(snapshot.now);
        app.on_collect(Ok(snapshot));
        app
    }

    /// A collect finished.
    pub fn on_collect(&mut self, result: Result<Snapshot, String>) {
        match result {
            Ok(s) => {
                self.snapshot = s;
                self.loaded = true;
                self.notice = None;
                self.land_pending();
            }
            Err(why) => self.notice = Some(why),
        }
    }

    /// Put the cursor on the session started from the form once it shows up; give up after
    /// [`PENDING_GRACE`] seconds.
    fn land_pending(&mut self) {
        let Some((job, since)) = &self.pending else { return };
        if self.now() - since > PENDING_GRACE {
            self.pending = None;
            return;
        }
        if let Some(s) = self.snapshot.sessions.iter().find(|s| s.job.as_ref() == Some(job)) {
            self.cursor = Some(Cursor::Session(s.session_id.clone()));
            self.pending = None;
        }
    }

    fn now(&self) -> f64 {
        self.clock.unwrap_or_else(crate::facts::now)
    }

    /// A [`Task`] finished.
    pub fn on_done(&mut self, done: Done) {
        match done {
            Done::Stopped { title, result } => match result {
                Ok(()) => self.toast(Tone::Ok, format!("Stopped “{title}”.")),
                Err(why) => self.toast(Tone::Error, short_error(&why)),
            },
            Done::Deleted { title, after, result } => match result {
                Ok(()) => {
                    self.toast(Tone::Ok, format!("Deleted “{title}”."));
                    if after.is_some() {
                        self.cursor = after;
                    }
                }
                Err(why) => self.toast(Tone::Error, short_error(&why)),
            },
            Done::Started { label, result } => match result {
                Ok(job) => {
                    self.toast(Tone::Ok, format!("Started “{label}” ({job})."));
                    self.pending = Some((job.clone(), self.now()));
                    self.effects.push(Effect::Enter(Enter::Attach { job }));
                }
                Err(why) => self.toast(Tone::Error, short_error(&why)),
            },
        }
        self.effects.push(Effect::Refresh);
    }

    /// Run the pending tasks now, on this thread, with `cli`, until none are left; return the
    /// other effects (attach, refresh) in order. For tests with a fake runner; the run loop does
    /// the same, with stop and delete on their own threads.
    pub fn run_tasks<R: Runner>(&mut self, cli: &Cli<R>) -> Vec<Effect> {
        let mut rest = Vec::new();
        loop {
            let effects = self.take_effects();
            if effects.is_empty() {
                return rest;
            }
            for e in effects {
                match e {
                    Effect::Run(task) => {
                        let done =
                            task.perform(cli, self.settings.deleted_path.as_deref(), self.settings.days, self.now());
                        self.on_done(done);
                    }
                    other => rest.push(other),
                }
            }
        }
    }

    /// The selected session's id, if the cursor is on one.
    pub fn selected(&self) -> Option<&str> {
        match &self.cursor {
            Some(Cursor::Session(id)) => Some(id),
            _ => None,
        }
    }

    pub fn toast(&mut self, tone: Tone, text: impl Into<String>) {
        let secs = if tone == Tone::Error { 8 } else { 4 };
        self.toasts.push_back(Toast { text: text.into(), tone, until: Instant::now() + Duration::from_secs(secs) });
        while self.toasts.len() > TOASTS {
            self.toasts.pop_front();
        }
    }

    pub fn toasts(&self) -> impl Iterator<Item = &Toast> {
        self.toasts.iter()
    }

    /// Drop toasts whose time is up; true if any went (so redraw).
    pub fn expire_toasts(&mut self) -> bool {
        let before = self.toasts.len();
        let now = Instant::now();
        self.toasts.retain(|t| t.until > now);
        self.toasts.len() != before
    }

    /// What the run loop should do; empties the list.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    /// True while the active view animates.
    pub fn animating(&self) -> bool {
        self.views[self.view.index()].animating(&cx!(self))
    }

    // --- keys ------------------------------------------------------------------------------------

    pub fn handle_key(&mut self, key: KeyEvent) {
        if key.kind == KeyEventKind::Release {
            return;
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            self.quit = true;
            return;
        }
        if let Some(overlay) = &mut self.overlay {
            match overlay.handle_key(key) {
                Reply::Keep => {}
                Reply::Close => self.overlay = None,
                Reply::Do(a) => {
                    self.overlay = None;
                    self.run(a);
                }
            }
            return;
        }
        let own = match key.code {
            KeyCode::Char('z') if ctrl => Some(Action::Toast(
                "Ctrl+Z is how you come back here from a session; the board itself isn't suspended. q quits.".into(),
            )),
            KeyCode::F(1) => Some(Action::Help),
            KeyCode::Tab => Some(Action::SwitchView(Switch::Next)),
            KeyCode::BackTab => Some(Action::SwitchView(Switch::Prev)),
            _ => None,
        };
        let action = own.or_else(|| {
            let cx = cx!(self);
            self.views[self.view.index()].handle_key(key, &cx, &mut self.cursor)
        });
        if let Some(a) = action.or_else(|| self.global_key(key)) {
            self.run(a);
        }
    }

    /// The keys every view shares, for keys the view didn't use. Letters only when the view
    /// isn't taking text; Ctrl chords always.
    fn global_key(&self, key: KeyEvent) -> Option<Action> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let selected = || self.selected().map(str::to_string);
        match key.code {
            KeyCode::Esc => return Some(Action::Back),
            KeyCode::Enter => return selected().map(Action::Jump),
            KeyCode::Char('s') if ctrl => return selected().map(Action::Stop),
            KeyCode::Char('x') if ctrl => return selected().map(Action::Delete),
            KeyCode::Char('e') if ctrl => return Some(Action::ToggleEnded),
            _ => {}
        }
        if ctrl || alt || self.views[self.view.index()].captures_text() {
            return None;
        }
        match key.code {
            KeyCode::Char('q') => Some(Action::Quit),
            KeyCode::Char('/') => Some(Action::OpenFinderSearch),
            KeyCode::Char('?') => Some(Action::Help),
            KeyCode::Char('e') => Some(Action::ToggleEnded),
            KeyCode::Char('r') => Some(Action::Refresh),
            KeyCode::Char('n') => Some(Action::New),
            KeyCode::Char('s') => selected().map(Action::Stop),
            KeyCode::Char('x') => selected().map(Action::Delete),
            _ => None,
        }
    }

    // --- actions ---------------------------------------------------------------------------------

    /// Do `action`. The one place actions run.
    pub fn run(&mut self, action: Action) {
        match action {
            Action::Handled => {}
            Action::Jump(id) => {
                if let Some(s) = self.session(&id) {
                    match actions::plan_enter(&s) {
                        Ok(how) => self.effects.push(Effect::Enter(how)),
                        Err(why) => self.toast(Tone::Warn, why.0),
                    }
                }
            }
            Action::Stop(id) => {
                let Some(s) = self.session(&id) else { return };
                match actions::plan_stop(&s) {
                    Err(why) => self.toast(Tone::Warn, why.0),
                    Ok(_) => match actions::stop_confirm(&s) {
                        Some(text) => {
                            let cmd = format!("{} stop {}", self.settings.claude.join(" "), s.job.clone().unwrap_or_default());
                            let c = Confirm::new("Stop?", text, "Stop", Action::StopConfirmed(id)).command(cmd);
                            self.overlay = Some(Overlay::Confirm(c));
                        }
                        None => self.run(Action::StopConfirmed(id)),
                    },
                }
            }
            Action::Delete(id) => {
                let Some(s) = self.session(&id) else { return };
                match actions::plan_delete(&s, self.settings.days) {
                    Err(why) => self.toast(Tone::Warn, why.0),
                    Ok(_) => {
                        let text = actions::delete_confirm(&s, self.settings.days);
                        let cmd = format!("{} rm {}", self.settings.claude.join(" "), s.job.clone().unwrap_or_default());
                        let c = Confirm::new("Delete?", text, "Delete", Action::DeleteConfirmed(id)).command(cmd);
                        self.overlay = Some(Overlay::Confirm(c));
                    }
                }
            }
            // Checked again: the session may have changed while the question was open.
            Action::StopConfirmed(id) => {
                let Some(s) = self.session(&id) else { return };
                match actions::plan_stop(&s) {
                    Ok(job) => {
                        self.toast(Tone::Info, format!("Stopping “{}”…", s.title));
                        self.effects.push(Effect::Run(Task::Stop { title: s.title.clone(), job }));
                    }
                    Err(why) => self.toast(Tone::Warn, why.0),
                }
            }
            Action::DeleteConfirmed(id) => {
                let Some(s) = self.session(&id) else { return };
                match actions::plan_delete(&s, self.settings.days) {
                    Ok(job) => {
                        let after = actions::after_delete(&self.board_groups(), &id);
                        self.toast(Tone::Info, format!("Deleting “{}”…", s.title));
                        self.effects.push(Effect::Run(Task::Delete {
                            session_id: id,
                            root: s.root.clone(),
                            title: s.title.clone(),
                            job,
                            after,
                        }));
                    }
                    Err(why) => self.toast(Tone::Warn, why.0),
                }
            }
            Action::New => {
                let folder = actions::default_folder(
                    self.cursor.as_ref(),
                    &self.snapshot.sessions,
                    &self.settings.scratch,
                    &self.settings.home,
                );
                self.overlay = Some(Overlay::Form(NewForm::new(&folder, &self.settings.home, &self.settings.claude)));
            }
            Action::StartNew(req) => {
                let where_ = widgets::short_path(&req.cwd, &self.settings.home);
                self.toast(Tone::Info, format!("Starting “{}” in {where_}…", new_label(&req)));
                self.effects.push(Effect::Run(Task::New(req)));
            }
            Action::Menu => {
                let menu = {
                    let cx = cx!(self);
                    super::views::finder::ActionMenu::new(&cx, self.cursor.as_ref())
                };
                self.overlay = Some(Overlay::Menu(menu));
            }
            Action::ToggleEnded => {
                self.prefs.show_ended = !self.prefs.show_ended;
                self.save_prefs();
                let text = if self.prefs.show_ended { "Showing ended sessions" } else { "Ended sessions hidden" };
                self.toast(Tone::Ok, text);
            }
            Action::ToggleColumns => {
                self.prefs.show_columns = !self.prefs.show_columns;
                self.save_prefs();
            }
            Action::ToggleFiles => {
                self.prefs.all_files = !self.prefs.all_files;
                self.save_prefs();
            }
            Action::SwitchView(to) => {
                let target = match to {
                    Switch::Next => self.view.next(),
                    Switch::Prev => self.view.prev(),
                    Switch::To(v) => v,
                };
                self.switch_to(target, false);
            }
            Action::OpenFinderSearch => self.switch_to(ViewId::Finder, true),
            Action::Back => {
                if let Some(v) = self.history.pop() {
                    self.view = v;
                    self.views[v.index()].shown(false);
                }
            }
            Action::Help => self.overlay = Some(Overlay::Help),
            Action::Refresh => self.effects.push(Effect::Refresh),
            Action::Toast(text) => self.toast(Tone::Info, text),
            Action::Quit => self.quit = true,
        }
    }

    /// The session with this id (a copy, so the app can change while it's in hand), or a
    /// toast saying it's gone.
    fn session(&mut self, id: &str) -> Option<Session> {
        let found = self.snapshot.get(id).cloned();
        if found.is_none() {
            self.toast(Tone::Warn, "That session isn't on the board any more.");
        }
        found
    }

    /// The board's rows as on screen: each project with its visible session ids in order. For
    /// where the cursor goes after a delete.
    fn board_groups(&self) -> Vec<(String, Vec<String>)> {
        let visible: Vec<Session> = cx!(self).visible().cloned().collect();
        group(&visible).into_iter().map(|(p, ss)| (p, ss.into_iter().map(|s| s.session_id.clone()).collect())).collect()
    }

    fn switch_to(&mut self, target: ViewId, search: bool) {
        if target != self.view {
            self.history.push(self.view);
            if self.history.len() > 16 {
                self.history.remove(0);
            }
            self.view = target;
        }
        self.views[target.index()].shown(search);
    }

    fn save_prefs(&self) {
        if let Some(path) = &self.settings.prefs_path {
            store::save_prefs(self.prefs, path);
        }
    }

    // --- drawing ---------------------------------------------------------------------------------

    pub fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let t = self.theme;
        widgets::fill(frame.buffer_mut(), area, t.text());
        if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
            let msg = widgets::trunc("Make the window bigger", usize::from(area.width));
            widgets::put_str(frame.buffer_mut(), area.x, area.y, &msg, t.text(), area.width);
            return;
        }
        let cx = cx!(self);
        self.views[self.view.index()].draw(frame, area, &cx, &mut self.cursor);

        let buf = frame.buffer_mut();
        let color = |tone| match tone {
            Tone::Info => t.accent,
            Tone::Ok => t.ok,
            Tone::Warn => t.warn,
            Tone::Error => t.error,
        };
        let mut lines: Vec<(&str, Color)> = Vec::new();
        if !self.loaded {
            lines.push(("Reading sessions…", t.accent));
        }
        if let Some(n) = &self.notice {
            lines.push((n, t.error));
        }
        lines.extend(self.toasts.iter().map(|x| (x.text.as_str(), color(x.tone))));
        // Newest at the bottom.
        for (row, (text, c)) in lines.iter().rev().enumerate() {
            widgets::toast(buf, &t, area, row as u16, text, *c);
        }
        if let Some(o) = &self.overlay {
            o.draw(buf, area, &t);
        }
    }
}
