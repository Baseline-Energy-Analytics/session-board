//! Overlays: dialogs that belong to the app, not a view. One is open at a time; it is drawn over
//! the view and takes every key (bar Ctrl+C, which always quits): the help, a yes/no confirm,
//! the new-session form and Finder's Ctrl+K action menu.

use super::app::Action;
use super::theme::{Theme, fg};
use super::views::finder::ActionMenu;
use super::widgets::{self, Edit, LineEdit, fill, popup, put, put_str, trunc, wrap};
use crate::actions::{self, NewSession};
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

pub enum Overlay {
    /// F1 (and `?` outside Finder): the keys.
    Help,
    /// A yes/no question; yes runs its action.
    Confirm(Confirm),
    /// `n` (or New session… in the menu): folder, name, first prompt.
    Form(NewForm),
    /// Ctrl+K in Finder: what can be done to the selected session.
    Menu(ActionMenu),
}

/// What an overlay did with a key.
#[derive(Debug, PartialEq)]
pub enum Reply {
    /// Stay open.
    Keep,
    Close,
    /// Close, then run this.
    Do(Action),
}

// --- confirm -------------------------------------------------------------------------------------

pub struct Confirm {
    pub title: String,
    /// The question, with any detail after a blank line.
    pub text: String,
    /// What yes does, one word: "Delete".
    pub yes: String,
    /// The command yes runs (`claude rm a1b2c3d4`), shown under the text; may be empty.
    pub command: String,
    pub then: Action,
    /// Whether the yes button has the focus. It starts on Cancel, so a stray Enter does nothing
    /// harmful.
    pub on_yes: bool,
}

impl Confirm {
    pub fn new(title: &str, text: String, yes: &str, then: Action) -> Confirm {
        Confirm { title: title.into(), text, yes: yes.into(), command: String::new(), then, on_yes: false }
    }

    pub fn command(mut self, command: String) -> Confirm {
        self.command = command;
        self
    }

    fn handle_key(&mut self, key: KeyEvent) -> Reply {
        match key.code {
            KeyCode::Char('y' | 'Y') => Reply::Do(self.then.clone()),
            KeyCode::Char('n' | 'N') | KeyCode::Esc => Reply::Close,
            KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => {
                self.on_yes = !self.on_yes;
                Reply::Keep
            }
            KeyCode::Enter if self.on_yes => Reply::Do(self.then.clone()),
            KeyCode::Enter => Reply::Close,
            _ => Reply::Keep,
        }
    }
}

// --- the new-session form ------------------------------------------------------------------------

/// The form's fields, in Tab order.
pub const FOLDER: usize = 0;
pub const NAME: usize = 1;
pub const PROMPT: usize = 2;

const LABELS: [&str; 3] = ["Folder", "Name", "First prompt (optional: without one it waits for you)"];
const PLACEHOLDERS: [&str; 3] = ["where it starts", "what this session is for", "what to do first"];

/// The new-session form: three line fields, the focused one taking every printable key.
pub struct NewForm {
    pub fields: [LineEdit; 3],
    pub focus: usize,
    /// Why the last Enter was refused (`actions::plan_new`).
    pub error: Option<String>,
    /// For `~` in the folder.
    home: String,
    /// The claude command as configured, for the command preview.
    claude: String,
}

impl NewForm {
    /// A form starting in `folder` (shown with `~`), the cursor in the name.
    pub fn new(folder: &str, home: &str, claude: &[String]) -> NewForm {
        NewForm {
            fields: [LineEdit::new(&widgets::short_path(folder, home)), LineEdit::default(), LineEdit::default()],
            focus: NAME,
            error: None,
            home: home.into(),
            claude: claude.join(" "),
        }
    }

    pub fn text(&self, field: usize) -> &str {
        self.fields[field].text()
    }

    /// Check the fields; the request, or why not.
    pub fn plan(&self) -> Result<NewSession, actions::Refused> {
        actions::plan_new(self.text(FOLDER), self.text(NAME), self.text(PROMPT), &self.home)
    }

    fn handle_key(&mut self, key: KeyEvent) -> Reply {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => return Reply::Close,
            KeyCode::Tab | KeyCode::Down => self.focus = (self.focus + 1) % 3,
            KeyCode::BackTab | KeyCode::Up => self.focus = (self.focus + 2) % 3,
            KeyCode::Char('n') if ctrl => self.focus = (self.focus + 1) % 3,
            KeyCode::Char('p') if ctrl => self.focus = (self.focus + 2) % 3,
            KeyCode::Enter => {
                return match self.plan() {
                    Ok(req) => Reply::Do(Action::StartNew(req)),
                    Err(why) => {
                        self.error = Some(why.0);
                        Reply::Keep
                    }
                };
            }
            _ => {
                if self.fields[self.focus].handle(&key) == Edit::Changed {
                    self.error = None;
                }
            }
        }
        Reply::Keep
    }

    /// The command the form would run, as far as it can tell: for the preview line.
    fn preview(&self) -> String {
        let name = self.text(NAME).split_whitespace().collect::<Vec<_>>().join(" ");
        let prompt = self.text(PROMPT).trim();
        let mut cmd = format!("{} --bg", self.claude);
        if !name.is_empty() {
            cmd.push_str(&format!(" --name {}", quote(&name)));
        }
        if !prompt.is_empty() {
            cmd.push_str(&format!(" -- {}", quote(prompt)));
        }
        cmd
    }
}

/// A word for a shell, quoted when it needs to be.
fn quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:@%+=,".contains(c)) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

// --- the overlay ---------------------------------------------------------------------------------

/// The help screen: a heading, or a key and what it does.
const HELP: &[(&str, &str)] = &[
    ("#", "Everywhere"),
    ("Enter", "jump in (an ended session is resumed in the background first)"),
    ("Ctrl+Z", "inside a session (or ← then Esc): back to the board"),
    ("Tab  Shift+Tab", "next / previous view: Columns → Timeline → Finder"),
    ("Esc", "close a dialog, clear the search, or go back; never quits"),
    ("F1", "this help (? too, outside Finder)"),
    ("Ctrl+C", "quit"),
    ("#", "Columns and Timeline"),
    ("↑ ↓  PgUp PgDn  Home End", "move"),
    ("n", "new background session (folder, name, first prompt), then jump in"),
    ("s  x", "stop / delete a background session (delete asks first)"),
    ("e", "show or hide ended sessions (remembered)"),
    ("/", "search, in Finder (ended sessions too)"),
    ("r  q", "refresh, quit"),
    ("← →", "Columns: between columns; Enter on a project moves into it"),
    ("c", "Columns: the metadata strip (remembered)"),
    ("t  1  7", "Timeline: toggle 24 hours / 7 days, or pick one"),
    ("#", "Finder (letters are search text)"),
    ("↑ ↓  Ctrl+P Ctrl+N", "move (PgUp PgDn, Home End too)"),
    ("← →  Ctrl+U  Ctrl+W", "in the search: move, clear it, delete a word"),
    ("Ctrl+K", "actions menu: each claude command, or why it can't be done; New session…"),
    ("Ctrl+S  Ctrl+X  Ctrl+E", "stop, delete, show or hide ended sessions"),
    ("#", "New session form"),
    ("Tab  Shift+Tab  ↑ ↓", "between folder, name and first prompt"),
    ("Enter  Esc", "start it and jump in / cancel (a name or a prompt is needed)"),
    ("", "folders don't tab-complete yet"),
];

impl Overlay {
    pub fn handle_key(&mut self, key: KeyEvent) -> Reply {
        match self {
            Overlay::Help => match key.code {
                KeyCode::Esc | KeyCode::F(1) | KeyCode::Enter | KeyCode::Char('?' | 'q') => Reply::Close,
                _ => Reply::Keep,
            },
            Overlay::Confirm(c) => c.handle_key(key),
            Overlay::Form(f) => f.handle_key(key),
            Overlay::Menu(m) => m.handle_key(key),
        }
    }

    pub fn draw(&self, buf: &mut Buffer, area: Rect, t: &Theme) {
        match self {
            Overlay::Help => draw_help(buf, area, t),
            Overlay::Confirm(c) => draw_confirm(buf, area, t, c),
            Overlay::Form(f) => draw_form(buf, area, t, f),
            Overlay::Menu(m) => m.draw(buf, area, t),
        }
    }
}

fn draw_help(buf: &mut Buffer, area: Rect, t: &Theme) {
    let keys_w = HELP.iter().filter(|(k, _)| *k != "#").map(|(k, _)| widgets::width(k)).max().unwrap_or(0) as u16;
    let text_w = HELP.iter().map(|(_, d)| widgets::width(d)).max().unwrap_or(0) as u16;
    let title = Line::from(Span::styled(" Keys ", t.bold(t.accent)));
    let footer = Line::from(Span::styled(" Esc or F1 closes ", fg(t.faint)));
    let inner = popup(buf, t, area, keys_w + text_w + 9, HELP.len() as u16 + 4, t.accent, title, footer);
    for (i, (k, d)) in HELP.iter().enumerate() {
        let y = inner.y + 1 + i as u16;
        if y >= inner.bottom() {
            break;
        }
        let x = inner.x + 2;
        if *k == "#" {
            put_str(buf, x, y, d, t.bold(t.accent).bg(t.raised), inner.width.saturating_sub(4));
            continue;
        }
        put_str(buf, x, y, k, t.bold(t.bright).bg(t.raised), keys_w);
        let room = inner.right().saturating_sub(x + keys_w + 3);
        let style = if k.is_empty() { fg(t.dim).add_modifier(Modifier::ITALIC) } else { fg(t.fg) };
        put_str(buf, x + keys_w + 3, y, &trunc(d, usize::from(room)), style.bg(t.raised), room);
    }
}

/// A button: ` Delete (y) `, lit when it has the focus.
fn button(t: &Theme, label: &str, focused: bool, danger: bool) -> Span<'static> {
    let style = match (focused, danger) {
        (true, true) => Style::new().fg(t.needs_ink).bg(t.warn).add_modifier(Modifier::BOLD),
        (true, false) => Style::new().fg(t.needs_ink).bg(t.accent).add_modifier(Modifier::BOLD),
        (false, _) => Style::new().fg(t.fg).bg(t.keycap),
    };
    Span::styled(format!(" {label} "), style)
}

fn draw_confirm(buf: &mut Buffer, area: Rect, t: &Theme, c: &Confirm) {
    let w = 68u16.min(area.width.saturating_sub(4));
    let text_w = usize::from(w.saturating_sub(6)).max(1);
    let lines = wrap(&c.text, text_w);
    let cmd = (!c.command.is_empty()).then(|| trunc(&format!("$ {}", c.command), text_w));
    let extra = cmd.as_ref().map_or(0, |_| 2);
    let title = Line::from(Span::styled(format!(" {} ", c.title), t.bold(t.warn)));
    let footer = Line::from(Span::styled(" ← → choose · ↵ answers · y / n / Esc ", fg(t.faint)));
    let inner = popup(buf, t, area, w, lines.len() as u16 + 6 + extra, t.warn, title, footer);
    let x = inner.x + 2;
    let iw = inner.width.saturating_sub(4);
    let mut y = inner.y + 1;
    for (i, l) in lines.iter().enumerate() {
        if y + 2 >= inner.bottom() {
            break;
        }
        let style = if i == 0 { t.bold(t.bright) } else { fg(t.fg) };
        put_str(buf, x, y, l, style.bg(t.raised), iw);
        y += 1;
    }
    if let Some(cmd) = cmd
        && y + 3 < inner.bottom()
    {
        put_str(buf, x, y + 1, &cmd, fg(t.code).bg(t.raised), iw);
    }
    let y = inner.bottom().saturating_sub(2);
    let gap = Span::styled("  ", Style::new().bg(t.raised));
    let row = [
        button(t, &format!("{} (y)", c.yes), c.on_yes, true),
        gap,
        button(t, "Cancel (n)", !c.on_yes, false),
    ];
    let bw = widgets::spans_width(&row) as u16;
    put(buf, inner.right().saturating_sub(bw + 2).max(x), y, &row, iw);
}

fn draw_form(buf: &mut Buffer, area: Rect, t: &Theme, f: &NewForm) {
    let w = 78u16.min(area.width.saturating_sub(4));
    let iw = w.saturating_sub(6);
    let error = f.error.as_deref().map(|e| wrap(e, usize::from(iw).max(1))).unwrap_or_default();
    let error_h = error.len().min(3) as u16;
    // Three label+field pairs with gaps, the command, the error, the buttons.
    let h = 1 + 3 * 3 + 2 + error_h + u16::from(error_h > 0) + 2 + 2;
    let title = Line::from(Span::styled(" New background session ", t.bold(t.accent)));
    let footer = Line::from(Span::styled(" Tab/↑↓ fields · ↵ start · Esc cancel ", fg(t.faint)));
    let inner = popup(buf, t, area, w, h, t.accent, title, footer);
    let x = inner.x + 2;
    let mut y = inner.y + 1;
    let room = |y: u16| y < inner.bottom();
    for (i, field) in f.fields.iter().enumerate() {
        let focused = i == f.focus;
        if room(y) {
            let style = if focused { t.bold(t.bright) } else { fg(t.dim) };
            put_str(buf, x, y, LABELS[i], style.bg(t.raised), iw);
        }
        if room(y + 1) {
            let bg = if focused { t.sel } else { t.bg };
            fill(buf, Rect::new(x, y + 1, iw, 1), Style::new().bg(bg));
            let mark = if focused { "› " } else { "  " };
            let fx = put_str(buf, x, y + 1, mark, t.bold(t.accent).bg(bg), 2);
            let fw = iw.saturating_sub(3);
            if field.is_empty() && !focused {
                put_str(buf, fx, y + 1, PLACEHOLDERS[i], fg(t.faint).bg(bg).add_modifier(Modifier::ITALIC), fw);
            } else {
                field.render(buf, fx, y + 1, fw, fg(t.bright).bg(bg), focused);
            }
        }
        y += 3;
    }
    if room(y) {
        let cmd = trunc(&format!("$ {}", f.preview()), usize::from(iw));
        put_str(buf, x, y, &cmd, fg(t.code).bg(t.raised), iw);
    }
    y += 2;
    for l in error.iter().take(usize::from(error_h)) {
        if room(y) {
            put_str(buf, x, y, &format!("✗ {l}"), t.bold(t.error).bg(t.raised), iw);
        }
        y += 1;
    }
    let by = inner.bottom().saturating_sub(2);
    if by > inner.y {
        let gap = Span::styled("  ", Style::new().bg(t.raised));
        let row = [button(t, "Start (Enter)", true, false), gap, button(t, "Cancel (Esc)", false, false)];
        let bw = widgets::spans_width(&row) as u16;
        put(buf, inner.right().saturating_sub(bw + 2).max(x), by, &row, iw);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::KeyModifiers;

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    #[test]
    fn confirm_answers_y_n_and_esc_and_enter_starts_on_cancel() {
        let mut o = Overlay::Confirm(Confirm::new("Delete?", "Delete it?".into(), "delete", Action::Quit));
        assert_eq!(o.handle_key(key(KeyCode::Char('z'))), Reply::Keep);
        assert_eq!(o.handle_key(key(KeyCode::Enter)), Reply::Close, "the focus starts on Cancel");
        assert_eq!(o.handle_key(key(KeyCode::Left)), Reply::Keep);
        assert_eq!(o.handle_key(key(KeyCode::Enter)), Reply::Do(Action::Quit));
        assert_eq!(o.handle_key(key(KeyCode::Char('y'))), Reply::Do(Action::Quit));
        assert_eq!(o.handle_key(key(KeyCode::Esc)), Reply::Close);
        assert_eq!(Overlay::Help.handle_key(key(KeyCode::F(1))), Reply::Close);
    }

    #[test]
    fn the_form_moves_between_fields_types_and_refuses_in_words() {
        let dir = std::env::temp_dir();
        let dir = dir.to_string_lossy();
        let mut f = NewForm::new(&dir, "/nonexistent-home", &["claude".into()]);
        assert_eq!(f.focus, NAME);
        assert_eq!(f.text(FOLDER), dir);
        assert_eq!(f.handle_key(key(KeyCode::Enter)), Reply::Keep);
        assert_eq!(f.error.as_deref(), Some("Give the session a name or a first prompt."));
        for c in "q x".chars() {
            f.handle_key(key(KeyCode::Char(c)));
        }
        assert!(f.error.is_none(), "typing clears the error");
        assert_eq!(f.text(NAME), "q x");
        assert_eq!(f.preview(), "claude --bg --name 'q x'");
        f.handle_key(key(KeyCode::Tab));
        assert_eq!(f.focus, PROMPT);
        f.handle_key(key(KeyCode::Tab));
        assert_eq!(f.focus, FOLDER);
        f.handle_key(key(KeyCode::BackTab));
        assert_eq!(f.focus, PROMPT);
        f.handle_key(key(KeyCode::Up));
        f.handle_key(key(KeyCode::Up));
        assert_eq!(f.focus, FOLDER);
        f.fields[FOLDER].set("/no/such/folder");
        assert_eq!(f.handle_key(key(KeyCode::Enter)), Reply::Keep);
        assert_eq!(f.error.as_deref(), Some("There's no folder /no/such/folder."));
        f.fields[FOLDER].set(&dir);
        match f.handle_key(key(KeyCode::Enter)) {
            Reply::Do(Action::StartNew(req)) => assert_eq!(req.name, "q x"),
            other => panic!("{other:?}"),
        }
        assert_eq!(f.handle_key(key(KeyCode::Esc)), Reply::Close);
    }

    #[test]
    fn dialogs_draw_at_any_size() {
        let t = Theme::default();
        let mut form = NewForm::new("/tmp", "/home/u", &["claude".into()]);
        form.error = Some("There's no folder /a/very/long/path/that/goes/on/and/on/and/on/and/on/for/ever.".into());
        let confirm = Confirm::new("Delete?", "Delete “x”?\n\nIt can't be undone.".into(), "Delete", Action::Quit)
            .command("claude rm a1b2c3d4".into());
        for (w, h) in [(1, 1), (20, 6), (30, 8), (80, 24), (160, 45)] {
            let mut buf = Buffer::empty(Rect::new(0, 0, w, h));
            let a = buf.area;
            draw_form(&mut buf, a, &t, &form);
            draw_confirm(&mut buf, a, &t, &confirm);
            draw_help(&mut buf, a, &t);
        }
    }
}
