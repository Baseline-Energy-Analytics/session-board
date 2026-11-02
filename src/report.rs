//! The non-interactive outputs: `board --list`, `--json` and `--dump`, and the small text
//! helpers (ages, paths, project names) the views will share.

use crate::facts::{Session, Snapshot, State, group};
use crate::mask::truncate_chars;
use serde::Serialize;
use std::path::Path;

/// How a row is labelled for people: "needs you", "in another terminal", "stopped"…
pub fn state_word(s: &Session) -> &'static str {
    if s.stopped {
        return "stopped";
    }
    match s.state {
        State::Elsewhere => "in another terminal",
        State::Stale => "not running",
        other => other.as_str(),
    }
}

pub fn glyph(state: State) -> &'static str {
    match state {
        State::NeedsYou => "◆",
        State::Running | State::Elsewhere => "●",
        State::Idle => "■",
        State::Stale => "◇",
        State::Ended => "○",
    }
}

/// "now" under 90 seconds, then minutes, hours, days.
pub fn ago(epoch: f64, now: f64) -> String {
    let s = now - epoch;
    if s < 90.0 {
        "now".into()
    } else if s < 3600.0 {
        format!("{}m", (s / 60.0).floor())
    } else if s < 86400.0 {
        format!("{}h", (s / 3600.0).floor())
    } else {
        format!("{}d", (s / 86400.0).floor())
    }
}

/// `path` with your home folder shown as `~` (only yours, from `$HOME`).
pub fn short_path(path: &str, home: &str) -> String {
    match path.strip_prefix(home) {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => format!("~{rest}"),
        _ => path.to_string(),
    }
}

/// A project's display name: its folder's name, or "~ (home)" and "scratch".
pub fn project_name(path: &str, home: &str, scratch: &str) -> String {
    if path == home {
        return "~ (home)".into();
    }
    if path == scratch {
        return "scratch".into();
    }
    match Path::new(path).file_name() {
        Some(n) if !path.ends_with('/') => n.to_string_lossy().into_owned(),
        _ => path.to_string(),
    }
}

/// `board --list`: the board as plain text, for scripts and quick checks.
pub fn list(sessions: &[Session], now: f64, home: &str, scratch: &str) -> String {
    let mut out = String::new();
    for (path, items) in group(sessions) {
        out.push_str(&format!("{}  ({})\n", project_name(&path, home, scratch), short_path(&path, home)));
        for s in items {
            let job = s.job.as_ref().map(|j| format!("  [{j}]")).unwrap_or_default();
            out.push_str(&format!(
                "  {} {}  — {}, {}{job}\n",
                glyph(s.state),
                truncate_chars(&s.title, 60),
                state_word(s),
                ago(s.last, now)
            ));
            if let Some(w) = &s.waiting
                && matches!(s.state, State::NeedsYou | State::Stale)
            {
                let verb = if s.state == State::NeedsYou { "needs" } else { "needed" };
                out.push_str(&format!("      {verb}: {}\n", truncate_chars(w, 100)));
            }
        }
    }
    out
}

/// One row of `board --json`: what identifies a session and its state, and deliberately no
/// transcript text (no asked, said, waiting or summary). The Claude Code mod reads this.
#[derive(Serialize)]
pub struct JsonRow<'a> {
    pub session_id: &'a str,
    pub job: Option<&'a str>,
    pub title: &'a str,
    pub state: State,
    pub stopped: bool,
    /// What a stale job still claims.
    pub was: Option<State>,
    pub project: &'a str,
    pub cwd: &'a str,
    /// Epoch seconds, to the millisecond.
    pub last: f64,
    pub model: Option<&'a str>,
    pub context: u64,
    pub output: u64,
}

impl<'a> From<&'a Session> for JsonRow<'a> {
    fn from(s: &'a Session) -> Self {
        JsonRow {
            session_id: &s.session_id,
            job: s.job.as_deref(),
            title: &s.title,
            state: s.state,
            stopped: s.stopped,
            was: s.was,
            project: &s.project,
            cwd: &s.cwd,
            last: (s.last * 1000.0).round() / 1000.0,
            model: s.model.as_deref(),
            context: s.context,
            output: s.output,
        }
    }
}

/// `board --json`: one object per row.
pub fn json(sessions: &[Session]) -> String {
    let rows: Vec<JsonRow> = sessions.iter().map(JsonRow::from).collect();
    serde_json::to_string(&rows).expect("rows serialise")
}

/// `board --dump`: the whole snapshot, transcript text included, in the shape `--shot --data`
/// will read (`mockups/sample.json`). For fixtures and debugging; never for the mod.
pub fn dump(snapshot: &Snapshot) -> String {
    serde_json::to_string_pretty(snapshot).expect("snapshot serialises")
}
