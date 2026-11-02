//! The three views. Each file owns one view; `README.md` beside this file says how a view
//! plugs in. Shared here: building the views, the board's row order and resolving the cursor.

pub mod columns;
pub mod finder;
pub mod timeline;

use super::app::{Ctx, View};
use crate::actions::Cursor;
use crate::facts::{Session, group};

/// One of each view, in [`ViewId::ALL`] order.
pub fn all() -> [Box<dyn View>; 3] {
    [Box::new(columns::Columns::default()), Box::new(timeline::Timeline::default()), Box::new(finder::Finder::default())]
}

/// The visible sessions in the board's order: by project (the most urgent project first), then
/// state and recency within each.
pub fn board_rows<'a>(cx: &Ctx<'a>) -> Vec<&'a Session> {
    let visible: Vec<Session> = cx.visible().cloned().collect();
    let order: Vec<String> = group(&visible).into_iter().flat_map(|(_, ss)| ss.into_iter().map(|s| s.session_id.clone())).collect();
    order.iter().filter_map(|id| cx.snap.get(id)).collect()
}

/// Where `cursor` is in `rows`: its session if shown, else the first row of its project, else
/// the first row. The cursor is moved onto that row's session. `None` (and the cursor left
/// alone) when there are no rows, so a selection survives an empty or loading snapshot.
pub fn resolve(rows: &[&Session], cursor: &mut Option<Cursor>) -> Option<usize> {
    if rows.is_empty() {
        return None;
    }
    let i = match cursor {
        Some(Cursor::Session(id)) => rows.iter().position(|s| s.session_id == *id),
        Some(Cursor::Project(p)) => rows.iter().position(|s| s.project == *p),
        None => None,
    }
    .unwrap_or(0);
    *cursor = Some(Cursor::Session(rows[i].session_id.clone()));
    Some(i)
}

/// Move the cursor `delta` rows (clamped) through `rows`.
pub fn step(rows: &[&Session], cursor: &mut Option<Cursor>, delta: isize) {
    if let Some(i) = resolve(rows, cursor) {
        let j = (i as isize + delta).clamp(0, rows.len() as isize - 1) as usize;
        *cursor = Some(Cursor::Session(rows[j].session_id.clone()));
    }
}
