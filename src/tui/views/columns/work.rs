//! The conversation column's two work cards: **Files** (what the session read and changed, from
//! its transcript) and **Git** (the state of the folder it works in, now).

use super::preview::boxed;
use crate::claude::transcript::FileTouch;
use crate::facts::Session;
use crate::git::GitInfo;
use crate::tui::app::Ctx;
use crate::tui::theme::fg;
use crate::tui::widgets::{ago, short_path, spans_width, trunc, width};
use ratatui::buffer::Buffer;
use ratatui::text::{Line, Span};

/// Changed files listed before "… N more".
const SHOWN: usize = 6;

/// `path` for display: relative to `folder` when under it, else with `~` for home.
pub fn rel_path(path: &str, folder: &str, home: &str) -> String {
    match path.strip_prefix(folder).and_then(|r| r.strip_prefix('/')) {
        Some(rest) if !folder.is_empty() && !rest.is_empty() => rest.to_string(),
        _ => short_path(path, home),
    }
}

/// `s` cut from the front to `w` cells, so a path keeps its file name: `…/auth/session.rs`.
pub fn tail(s: &str, w: usize) -> String {
    if width(s) <= w {
        return s.to_string();
    }
    let mut out: Vec<char> = Vec::new();
    let mut used = 1;
    for c in s.chars().rev() {
        let cw = width(c.encode_utf8(&mut [0; 4]));
        if used + cw > w {
            break;
        }
        used += cw;
        out.push(c);
    }
    out.reverse();
    format!("…{}", out.into_iter().collect::<String>())
}

/// Files read but not changed, folded by folder: `src/auth/ (6) · Cargo.toml`.
pub fn read_summary(files: &[&FileTouch], folder: &str, home: &str) -> String {
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for f in files {
        let rel = rel_path(&f.path, folder, home);
        let (dir, name) = match rel.rsplit_once('/') {
            Some((d, n)) => (format!("{d}/"), n.to_string()),
            None => (String::new(), rel.clone()),
        };
        match groups.iter_mut().find(|(d, _)| *d == dir) {
            Some((_, names)) => names.push(name),
            None => groups.push((dir, vec![name])),
        }
    }
    groups.sort_by_key(|(_, n)| std::cmp::Reverse(n.len()));
    groups
        .into_iter()
        .map(|(dir, names)| if names.len() == 1 { format!("{dir}{}", names[0]) } else { format!("{dir} ({})", names.len()) })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// The Files card; returns the row after it (`y` when the session touched no files). Shows the
/// latest few changed files and the read ones folded by folder, or with `f` (`all_files`) every
/// file, one per line.
pub(super) fn files_card(buf: &mut Buffer, cx: &Ctx, x: u16, y: u16, w: u16, s: &Session) -> u16 {
    if s.files.is_empty() {
        return y;
    }
    let t = cx.theme;
    let all = cx.prefs.all_files;
    let folder = s.folder();
    let (changed, read): (Vec<&FileTouch>, Vec<&FileTouch>) = s.files.iter().partition(|f| f.changed());
    let inner = usize::from(w.saturating_sub(6)).max(16);
    let mut body: Vec<Vec<Span>> = Vec::new();
    let shown = if all { changed.len() } else { SHOWN };
    for f in changed.iter().take(shown) {
        let (glyph, color) = if f.created {
            ("+", t.ok)
        } else if f.edits > 0 {
            ("✎", t.claude)
        } else {
            ("$", t.elsewhere)
        };
        let mut right = Vec::new();
        if f.added > 0 {
            right.push(Span::styled(format!("+{}", f.added), fg(t.ok)));
        }
        if f.removed > 0 {
            if !right.is_empty() {
                right.push(Span::raw(" "));
            }
            right.push(Span::styled(format!("−{}", f.removed), fg(t.error)));
        }
        right.push(Span::styled(format!("{:>5}", ago(f.last as f64, cx.now)), fg(t.dim)));
        body.push(file_line(cx, inner, Span::styled(format!("{glyph} "), fg(color)), &rel_path(&f.path, folder, cx.home), right));
    }
    if changed.len() > shown {
        body.push(vec![Span::styled(format!("… {} more changed", changed.len() - shown), fg(t.dim))]);
    }
    if !read.is_empty() {
        if all {
            if !changed.is_empty() {
                body.push(Vec::new());
            }
            for f in &read {
                let right = vec![Span::styled(format!("{:>5}", ago(f.last as f64, cx.now)), fg(t.dim))];
                body.push(file_line(cx, inner, Span::styled("· ", fg(t.faint)), &rel_path(&f.path, folder, cx.home), right));
            }
        } else {
            let label = "read  ";
            let summary = trunc(&read_summary(&read, folder, cx.home), inner.saturating_sub(label.len()));
            body.push(vec![Span::styled(label, fg(t.faint)), Span::styled(summary, fg(t.dim))]);
        }
    }
    let mut title = vec![Span::raw(" "), Span::styled("Files", t.bold(t.bright))];
    let mut counts = Vec::new();
    if !changed.is_empty() {
        let (a, d) = changed.iter().fold((0, 0), |(a, d), f| (a + f.added, d + f.removed));
        counts.push(format!("{} changed  +{a} −{d}", changed.len()));
    }
    if !read.is_empty() {
        counts.push(format!("{} read", read.len()));
    }
    title.push(Span::styled(format!(" · {} ", counts.join(" · ")), fg(t.dim)));
    let folded = changed.len() > SHOWN || !read.is_empty();
    if all || folded {
        title.extend([Span::styled("· ", fg(t.faint)), Span::styled("f", t.bold(t.bright))]);
        title.push(Span::styled(if all { " fewer " } else { " all files " }, fg(t.dim)));
    }
    boxed(buf, x, y, w, t.border, t.bg, Line::from(title), &body)
}

/// One file row: a glyph, the path (cut from the front to fit) and right-aligned `right`.
fn file_line<'a>(cx: &Ctx, inner: usize, glyph: Span<'a>, path: &str, right: Vec<Span<'a>>) -> Vec<Span<'a>> {
    let t = cx.theme;
    let rw = spans_width(&right);
    let path = tail(path, inner.saturating_sub(rw + 4).max(6));
    let gap = inner.saturating_sub(2 + width(&path) + rw).max(1);
    let mut line = vec![glyph, Span::styled(path, fg(t.fg)), Span::raw(" ".repeat(gap))];
    line.extend(right);
    line
}

/// The Git card; returns the row after it (`y` when the folder isn't in a git repository).
pub(super) fn git_card(buf: &mut Buffer, cx: &Ctx, x: u16, y: u16, w: u16, s: &Session) -> u16 {
    let Some(g) = &s.git else { return y };
    let t = cx.theme;
    let inner = usize::from(w.saturating_sub(6)).max(16);
    let dim = |v: String| Span::styled(v, fg(t.dim));
    let dot = || Span::styled("  ·  ", fg(t.faint));
    let mut body: Vec<Vec<Span>> = Vec::new();
    if let Some(key) = &g.refused {
        body.push(vec![dim(trunc(&format!("Not read: this repository's own config sets {key},"), inner))]);
        body.push(vec![dim("which git status could run.".into())]);
        let title = Line::from(vec![Span::raw(" "), Span::styled("Git", t.bold(t.bright)), Span::raw(" ")]);
        return boxed(buf, x, y, w, t.border, t.bg, title, &body);
    }

    // Upstream: ahead / behind, as of the last fetch.
    let mut line = Vec::new();
    match &g.upstream {
        Some(up) => {
            line.push(dim(format!("{up}  ")));
            match (g.ahead, g.behind) {
                (0, 0) => line.push(Span::styled("✓ in step", fg(t.ok))),
                (a, b) => {
                    if a > 0 {
                        line.push(Span::styled(format!("↑{a} to push"), fg(t.accent)));
                    }
                    if b > 0 {
                        if a > 0 {
                            line.push(Span::raw("  "));
                        }
                        line.push(Span::styled(format!("↓{b} to pull"), fg(t.code)));
                    }
                }
            }
            line.push(dot());
            line.push(dim(match g.fetched {
                Some(f) => format!("fetched {}", ago_long(f as f64, cx.now)),
                None => "never fetched".into(),
            }));
        }
        None if g.branch.is_some() => line.push(dim("no upstream: not pushed anywhere".into())),
        None => line.push(dim("detached HEAD".into())),
    }
    body.push(line);

    // The working tree.
    body.push(dirty_line(cx, g));

    // The last commit.
    if let (Some(at), Some(subject)) = (g.commit_at, &g.commit) {
        let lead = format!("last commit {}  ", ago_long(at as f64, cx.now));
        let room = inner.saturating_sub(width(&lead));
        body.push(vec![dim(lead), Span::styled(trunc(subject, room), fg(t.fg))]);
    }

    // Worktrees, and which one this is when there are several.
    if g.worktrees > 1 {
        let mut l = vec![Span::styled(format!("{} worktrees", g.worktrees), fg(t.fg))];
        if g.root != s.project {
            l.push(dot());
            l.push(dim(format!("this one: {}", short_path(&g.root, cx.home))));
        }
        body.push(l);
    }

    let branch = g.branch.clone().unwrap_or_else(|| "(detached)".into());
    let title = Line::from(vec![
        Span::raw(" "),
        Span::styled("Git", t.bold(t.bright)),
        Span::styled(" · ⎇ ", fg(t.faint)),
        Span::styled(trunc(&branch, inner.saturating_sub(12)), t.bold(t.accent)),
        Span::raw(" "),
    ]);
    boxed(buf, x, y, w, t.border, t.bg, title, &body)
}

/// "✓ nothing uncommitted", or the uncommitted counts.
fn dirty_line(cx: &Ctx, g: &GitInfo) -> Vec<Span<'static>> {
    let t = cx.theme;
    if g.dirty() == 0 {
        return vec![Span::styled("✓ nothing uncommitted", fg(t.ok))];
    }
    let mut parts = Vec::new();
    for (n, what) in [(g.conflicts, "in conflict"), (g.staged, "staged"), (g.modified, "modified"), (g.untracked, "untracked")] {
        if n > 0 {
            parts.push(format!("{n} {what}"));
        }
    }
    let color = if g.conflicts > 0 { t.error } else { t.code };
    vec![Span::styled("● uncommitted  ", fg(color)), Span::styled(parts.join(" · "), fg(t.fg))]
}

/// "just now", "5m ago".
fn ago_long(epoch: f64, now: f64) -> String {
    match ago(epoch, now).as_str() {
        "now" => "just now".into(),
        a => format!("{a} ago"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_relative_and_cut_from_the_front() {
        assert_eq!(rel_path("/h/u/app/src/a.rs", "/h/u/app", "/h/u"), "src/a.rs");
        assert_eq!(rel_path("/h/u/other/a.rs", "/h/u/app", "/h/u"), "~/other/a.rs");
        assert_eq!(rel_path("/h/u/app", "/h/u/app", "/h/u"), "~/app");
        assert_eq!(tail("src/auth/session.rs", 12), "…/session.rs");
        assert_eq!(tail("a.rs", 12), "a.rs");
    }

    #[test]
    fn reads_fold_by_folder() {
        let f = |p: &str| FileTouch { path: p.into(), read: true, ..FileTouch::default() };
        let files = [f("/r/src/a.rs"), f("/r/src/b.rs"), f("/r/Cargo.toml"), f("/r/src/c.rs")];
        let refs: Vec<&FileTouch> = files.iter().collect();
        assert_eq!(read_summary(&refs, "/r", "/h"), "src/ (3) · Cargo.toml");
    }
}
