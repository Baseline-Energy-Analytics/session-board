//! `daemon/roster.json`: the background workers Claude Code has a process for. The file also holds
//! socket auth tokens, so it is never printed or cached; only job ids leave this module.

use super::{read_json, registry};
use std::collections::HashSet;
use std::path::Path;

/// The jobs with a live worker, or `None` when that can't be told (no roster, as before the
/// background service first ran). The supervisor restarts a worker that dies, but after the
/// supervisor itself goes (a reboot, a crash) no job runs until it is opened.
pub fn live_workers(claude_dir: &Path) -> Option<HashSet<String>> {
    let roster = read_json(&claude_dir.join("daemon").join("roster.json"))?;
    let workers = roster.get("workers")?.as_object()?;
    Some(
        workers
            .iter()
            .filter(|(_, w)| w.is_object() && registry::alive(w.get("pid"), w.get("procStart")))
            .map(|(job, _)| job.clone())
            .collect(),
    )
}
