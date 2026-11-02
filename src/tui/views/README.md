# Writing a view

Each view is one file here (`columns.rs`, `timeline.rs`, `finder.rs`) holding one struct that
implements `View` (`src/tui/app.rs`). `app.rs` and `views/mod.rs` need no change unless your
struct stops being `Default` (then edit `views::all()`).

## The trait

```rust
pub trait View {
    fn draw(&mut self, frame: &mut Frame, area: Rect, cx: &Ctx, cursor: &mut Option<Cursor>);
    fn handle_key(&mut self, key: KeyEvent, cx: &Ctx, cursor: &mut Option<Cursor>) -> Option<Action>;
    fn captures_text(&self) -> bool { false }          // Finder: true
    fn shown(&mut self, search: bool) {}               // switched to; search = came via `/`
    fn animating(&self, cx: &Ctx) -> bool { /* any visible running session */ }
}
```

- **Your struct is your view state**: focus, scroll, per-project selection, query, range. It
  lives as long as the app, across view switches.
- **`cursor` is shared** by all three views: `Some(Cursor::Session(id))` or (Columns only)
  `Some(Cursor::Project(path))`, from `crate::actions::Cursor`. It is keyed by id, never by
  position: resolve it to a row *in `draw`*, from the snapshot as it is now, so a refresh that
  reorders rows never moves it. `views::resolve(&rows, cursor)` does the usual thing (its
  session, else the first row of its project, else the first row, and writes the result back;
  leaves it alone when there are no rows). Set it from `handle_key` when the user moves. Another
  view may hand you a `Project` cursor or an id you don't show (an ended session while you hide
  ended ones): fall back sensibly, don't panic.
- **`Ctx`** has `snap` (the `Snapshot`), `theme`, `now` (measure ages against this, not the
  clock), `home` and `scratch` (for `short_path`/`project_name`), `days`, `prefs`
  (`show_ended`, `show_columns`), `tick` (animation frame, +1 per 250 ms while `animating`),
  `loaded` (false until the first snapshot), `claude` (the configured command's words, e.g.
  `ssh box claude`; `cx.claude_cmd()` joins them, for showing commands), and `cx.visible()` (sessions less hidden ended
  ones, sorted by state then recency). `views::board_rows(cx)` gives them grouped by project in
  the board's order.
- **`area`** is the whole screen, at least `MIN_WIDTH` x `MIN_HEIGHT` (30x8); smaller and the app
  draws a note instead of calling you. Draw everything in it: header, body, footer. Paint your
  background (the app fills it with `theme.bg` first). Use `widgets::*`, which clip to the buffer;
  never index `buf[(x, y)]` without checking it's inside.

## Keys

The order is: Ctrl+C (always quits) → an open overlay (takes everything) → the app's own keys
(**Tab / Shift+Tab** switch views, **F1** help, **Ctrl+Z** a hint) → **your `handle_key`** → the
global table for keys you return `None` for:

| key | global meaning |
|---|---|
| Esc | back to the previous view (never quits) |
| Enter | `Jump(selected session)` |
| Ctrl+S / Ctrl+X / Ctrl+E | stop / delete / toggle ended |
| `q` `/` `?` `e` `r` `n` `s` `x` | quit, Finder search, help, ended, refresh, new, stop, delete; **only when `captures_text()` is false** |

Return `None` for keys you don't use, `Some(Action::Handled)` for keys you used yourself (a
cursor move, typing), or another `Action` to have the app do something. Override a global key by
handling it first (Columns' Enter on the projects column moves focus instead of jumping; Finder's
Esc clears a non-empty query and only then returns `None`). You never see Tab, BackTab, F1,
Ctrl+C or Ctrl+Z. Arrows only for movement; no vim keys.

## Actions

`Action` (in `app.rs`) carries session ids, never positions; the app runs it in one place:
`Jump(id)`, `Stop(id)`, `Delete(id)`, `New`, `Menu`, `ToggleEnded`, `ToggleColumns`,
`SwitchView(Switch::{Next, Prev, To(ViewId)})`, `Back`, `OpenFinderSearch`, `Help`, `Refresh`,
`Toast(text)`, `Quit`, `Handled`. Stop and Delete check `actions.rs` and confirm before anything
runs (stop only while the session is working; delete always, the focus on Cancel), so just send
them. The claude commands run as `Effect::Run(Task)`: stop and delete on their own thread, the
result coming back as a `Done` (toast, refresh, and after a delete the cursor moves to the next
session in the project, else the previous, else the project). `New` opens the form; its Enter
starts the session (`claude --bg`), attaches to it, and when you come back the cursor lands on
it as soon as a snapshot shows it (30 s grace).

**Whether something is allowed** is `crate::actions`' call, never yours: `plan_enter(s)`,
`plan_stop(s)`, `plan_delete(s, cx.days)` return `Ok(job …)` or `Err(Refused(why))` in plain
words. Use them to grey out a hint or list a refusal in a menu (Finder's Ctrl+K); the commands
the menu shows are `<claude> attach <job>`, `<claude> --bg --resume <id>`, `<claude> stop <job>`,
`<claude> rm <job>`, with `<claude>` the configured command (`cx.claude_cmd()`).

A pending tool call is `s.pending_tool` (`Some(PendingTool { name, arg })`, from the
transcript, masked); `s.waiting` is its text for display (or Claude Code's own words from
`state.json`, which have no `pending_tool`). Split "Tool: cmd" from `pending_tool`, never by
guessing at the string.

Overlays (help, confirm, the new-session form, Finder's Ctrl+K menu) are the app's
(`overlays.rs`, an enum); to add one, add a variant and an `Action` that opens it.

## Look

`cx.theme` (`theme.rs`) is the one palette: surfaces `bg strip bar sel_blur raised track sel
keycap sep border`, text `faint dim fg bright accent`, states via `theme.state(State)` (amber
`needs`, with `needs_ink needs_bg needs_sel`; `running elsewhere idle stale ended`, `stale_bg`),
conversation `you_bg claude claude_bg code`, feedback `ok warn error`. Compute colours only with
`theme.mix(a, b, t)`, `theme.darken(c, f)`, `theme.tint(state)`, `theme.gauge(t)`, so they
degrade in 256-colour terminals; never write `Color::Rgb` in a view.

`widgets.rs`: `trunc wrap width tokens model_name spinner` and (from `report.rs`) `ago
short_path project_name state_word glyph`; `put put_str put_right fit spans_width`; `fill hline
vline caps centered`; `pill state_pill` (a session's state in the board's words: NEEDS YOU, RUNNING, IN ANOTHER
TERMINAL, IDLE/STOPPED, NOT RUNNING, ENDED; spinner with `Some(tick)`) `keycap key_hints view_tabs`; `bar` (eighth blocks with a
per-cell colour) and `context_gauge` (tokens, a bar against `CONTEXT_SCALE` = 1M, labelled "of
1M"; not a percentage of the model's window); `panel popup toast scrollbar bubble`; and
`LineEdit` (Finder's query, the form's fields: `handle(&key) -> Edit::{Ignored, Moved,
Changed}`, `render`). If you need a helper another view could use, add it there rather than in
your file.

Toasts and the collect-failure notice are drawn by the app at the bottom right, over your view.

## Screenshots

```
cargo run -q -- --shot /tmp/x.svg --size 160x45 --view finder \
    --data tests/fixtures/sample.json --select a1 --keys a,u,down
```

`--keys` takes names: `a`, `?`, `enter`, `esc`, `tab`, `backtab`, `up`, `pgdn`, `home`, `f1`,
`space`, `comma`, `ctrl+k`, `shift+tab`. A frame is drawn before each key, as in the real
loop; keys can open overlays (`n` the form, `ctrl+k` in Finder the menu, `x` a confirm), and
nothing is ever run. `--home /home/u` shows the fixture's paths with `~` without setting `HOME`. `SESSION_BOARD_COLOR=256` previews the 256-colour degrade. To look at one,
`/opt/google/chrome/chrome --headless=new --screenshot=/tmp/x.png --window-size=1440,855
file:///tmp/x.svg` (9x19 px per cell). In tests, `board::shot::render(&mut app, w, h, &keys)`
returns the `Buffer`; `tests/tui.rs` shows the pattern (`App::offline(snapshot, home, scratch,
Theme::default())`). Add your view's own tests at 160x45 and 80x40 on the sample fixture;
the resize sweep in `tests/tui.rs` already drives every view from 40x12 to 240x70.
