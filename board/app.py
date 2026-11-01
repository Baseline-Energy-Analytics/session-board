"""The board: projects, then sessions, then a summary. Enter jumps into a session; Ctrl+Z inside it
(or ← then Esc) comes back here, to the same spot. n starts a session, s stops one, x deletes one,
/ filters."""
from __future__ import annotations

import json
import os
import re
import socket
import subprocess
import sys
import textwrap
import time

from rich.text import Text
from textual import work
from textual.app import App, ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical
from textual.screen import ModalScreen
from textual.suggester import Suggester
from textual.widgets import Button, Footer, Header, Input, Label, Static, Tree

from . import actions, facts
from .facts import HOME, Session

GLYPH = {"needs you": "◆", "running": "●", "elsewhere": "●", "idle": "■", "stale": "◇", "ended": "○"}
LABEL = {"elsewhere": "in another terminal", "stale": "not running"}
NEEDS_YOU = "bold yellow"  # the only colour on the board: it marks what needs you
POLL_SECONDS = facts.CONFIG.poll
STATE_WIDTH = 26    # "in another terminal · 13h"
COLUMNS_WIDTH = 32  # " 182k ctx  Opus 5.5    310k out"
COLUMNS_MIN = 100   # narrower than this, the columns stay hidden
SCRATCH = facts.SCRATCH


def ago(epoch: float, now: float | None = None) -> str:
    s = (time.time() if now is None else now) - epoch
    if s < 90:
        return "now"
    if s < 3600:
        return f"{int(s // 60)}m"
    if s < 86400:
        return f"{int(s // 3600)}h"
    return f"{int(s // 86400)}d"


def short_path(path: str) -> str:
    return "~" + path[len(HOME):] if path == HOME or path.startswith(HOME + "/") else path


def project_name(path: str) -> str:
    if path == HOME:
        return "~ (home)"
    if path == SCRATCH:
        return "scratch"
    return os.path.basename(path) or path


def state_word(s: Session) -> str:
    return "stopped" if s.stopped else LABEL.get(s.state, s.state)


def model_name(model: str | None) -> str:
    """claude-opus-5-5 → Opus 5.5; claude-sonnet-4-5-20250929 → Sonnet 4.5."""
    if not model:
        return ""
    m = re.match(r"claude-([a-z]+)-(\d+)(?:-(\d{1,2}))?(?!\d)", model)
    if not m:
        return model[:10]
    return f"{m.group(1).capitalize()} {m.group(2)}" + (f".{m.group(3)}" if m.group(3) else "")


def tokens(n: int) -> str:
    if n < 1000:
        return str(n)
    if n < 1_000_000:
        return f"{round(n / 1000)}k"
    return f"{n / 1_000_000:.1f}M"


def columns(s: Session) -> str:
    """Context, model and output tokens, fixed width so rows line up."""
    if not s.model:
        return " " * COLUMNS_WIDTH
    return f" {tokens(s.context):>5} ctx  {model_name(s.model):<10} {tokens(s.output):>5} out".ljust(COLUMNS_WIDTH)


def matches(s: Session, terms: list[str]) -> bool:
    """The filter: every word must appear in the title, the folder or the project's name."""
    hay = " ".join((s.title, s.cwd, short_path(s.cwd), s.project, project_name(s.project))).lower()
    return all(t in hay for t in terms)


def project_label(path: str, sessions: list[Session]) -> Text:
    counts = {}
    for s in sessions:
        counts[s.state] = counts.get(s.state, 0) + 1
    t = Text(project_name(path), style="bold")
    t.append(f"  {short_path(path)}", style="dim")
    parts = []
    for state in facts.STATES:
        if counts.get(state):
            parts.append((f"{counts[state]} {LABEL.get(state, state)}", NEEDS_YOU if state == "needs you" else "dim"))
    for i, (text, style) in enumerate(parts):
        t.append("  ·  " if i else "    ", style="dim")
        t.append(text, style=style)
    t.append(f"  ·  {ago(max(s.last for s in sessions))}", style="dim")
    return t


def _cut(text: str, room: int) -> str:
    return text if len(text) <= room else text[:max(0, room - 1)] + "…"


def session_label(s: Session, width: int, show_columns: bool = True) -> Text:
    """glyph, title (and what it is waiting on), state and age, then context, model and tokens."""
    hot = s.state == "needs you"
    quiet = s.state in ("ended", "stale")
    cols = show_columns and width >= COLUMNS_MIN
    room = max(20, width - 4 - STATE_WIDTH - (COLUMNS_WIDTH if cols else 0))
    style = NEEDS_YOU if hot else ("dim" if quiet else "")
    t = Text(f"{GLYPH[s.state]} ", style=style)
    title = _cut(s.title, room)
    if s.waiting and s.state in ("needs you", "stale"):
        title = _cut(s.title, max(24, room // 2))
        rest = room - len(title) - 3
        t.append(title, style=style)
        if rest >= 8:
            title += " — " + _cut(s.waiting, rest)
            t.append(" — " + _cut(s.waiting, rest), style="yellow italic" if hot else "dim italic")
    else:
        t.append(title, style=style)
    t.append(" " * (room - len(title)))
    t.append(f"  {state_word(s)} · {ago(s.last)}".ljust(STATE_WIDTH), style=NEEDS_YOU if hot else "dim")
    if cols:
        t.append(columns(s), style="dim")
    return t


def detail_lines(s: Session, width: int) -> list[Text]:
    """The summary under an expanded session, wrapped to the tree's width."""
    width = max(30, width)
    out: list[Text] = []

    def block(label: str, text: str | None, max_lines: int):
        if not text:
            return
        flat = " ".join(text.split())
        lines = textwrap.wrap(flat, width=width - len(label) - 1) or [""]
        if len(lines) > max_lines:
            lines = lines[:max_lines]
            lines[-1] = lines[-1][: max(0, width - len(label) - 2)] + "…"
        for i, line in enumerate(lines):
            row = Text((label if i == 0 else " " * len(label)) + " ", style="dim")
            row.append(line)
            out.append(row)

    if s.waiting:
        block("Needs:" if s.state == "needs you" else "Needed:", s.waiting, 3)
    block("Asked:", s.asked, 2)
    if s.summary and not (s.waiting and " ".join(s.summary.split()) in s.waiting):
        block("Status:", s.summary, 4)
    elif s.summary:
        pass  # Claude Code's status repeats what it needs, shown above
    else:
        block("Last:", s.said, 4)
    if s.cwd != s.project:
        out.append(Text(f"In:     {short_path(s.cwd)}", style="dim"))
    if s.model:
        out.append(Text(f"Model:  {model_name(s.model)} · {tokens(s.context)} in context · "
                        f"{tokens(s.output)} tokens written", style="dim"))
    if s.state == "stale":
        said = "it needs you" if s.was == "needs you" else "it is working"
        hint = (f"Not running: Claude Code has no process for it, so “{said}” is out of date. "
                "Enter: jump in, which starts it again. x: delete it.")
    elif s.state == "elsewhere":
        hint = "Open in another terminal; it can't be entered from here."
    elif s.state == "ended":
        hint = "Enter: resume it in the background and jump in."
    elif s.stopped:
        hint = "Stopped. Enter: open it again. x: delete it."
    else:
        hint = "Enter: jump in. s: stop it. x: delete it."
    out.append(Text(hint, style="dim italic"))
    return out


class SessionTree(Tree):
    BINDINGS = [
        Binding("enter", "select_cursor", "Jump in"),
        Binding("right", "expand", "Open", key_display="→"),
        Binding("left", "collapse", "Close", key_display="←"),
        Binding("n", "app.new_session", "New"),
        Binding("s", "app.stop", "Stop"),
        Binding("x", "app.delete", "Delete"),
        Binding("slash", "app.filter", "Filter", key_display="/"),
        Binding("e", "app.toggle_ended", "Ended"),
        Binding("c", "app.toggle_columns", "Columns"),
    ]

    def on_resize(self, event) -> None:
        # Rows are cut to the width they were drawn at: draw them again at the new one.
        if getattr(self.app, "_loaded", False) and event.size.width != self._drawn_width:
            self._drawn_width = event.size.width
            self.app.call_after_refresh(self.app.redraw)

    _drawn_width = 0

    def action_expand(self) -> None:
        node = self.cursor_node
        if node and node.allow_expand and not node.is_expanded:
            node.expand()
        elif node and node.children:
            self.action_cursor_down()

    def action_collapse(self) -> None:
        node = self.cursor_node
        if node is None:
            return
        if node.is_expanded and node.children:
            node.collapse()
        elif node.parent is not None and not node.parent.is_root:
            self.move_cursor(node.parent)


class Runner:
    """Runs the claude CLI for the board. Tests swap in a fake."""

    def resume(self, session_id: str, cwd: str) -> str:
        return actions.resume_in_background(session_id, cwd)

    def new(self, req: actions.NewSession) -> str:
        return actions.start_in_background(req)

    def stop(self, job: str) -> None:
        actions.stop_background(job)

    def delete(self, s: Session, job: str) -> None:
        actions.delete_background(job)
        try:
            facts.forget(s.session_id, s.root)  # `claude rm` keeps the transcript; the board hides it
        except OSError:
            pass  # the job is gone either way; its conversation just shows again as ended

    def attach(self, app: App, job: str) -> None:
        argv, cwd = actions.attach_argv(job)
        with app.suspend():
            subprocess.call(argv, cwd=cwd)


# --- dialogs -----------------------------------------------------------------------------------

DIALOG_CSS = """
ConfirmScreen, NewSessionScreen { align: center middle; }
#dialog { width: 76; max-width: 100%; height: auto; border: round $foreground 40%; background: $surface;
          padding: 1 2; }
#dialog Label { width: 100%; }
#dialog .field { margin-top: 1; color: $text-muted; }
#dialog Input { width: 100%; }
#dialog #error { text-style: bold; height: auto; margin-top: 1; }
#dialog #buttons { height: auto; width: 100%; align-horizontal: right; margin-top: 1; }
#dialog Button { margin-left: 2; min-width: 10; }
"""


class ConfirmScreen(ModalScreen[bool]):
    """Yes or no. The safe answer has the focus, so a stray Enter does nothing harmful."""
    BINDINGS = [
        Binding("y", "answer(True)", "Yes", show=False),
        Binding("n,escape", "answer(False)", "No", show=False),
    ]

    def __init__(self, message: str, yes: str):
        super().__init__()
        self.message, self.yes = message, yes

    def compose(self) -> ComposeResult:
        with Vertical(id="dialog"):
            yield Label(self.message, markup=False)  # titles may hold [brackets]
            with Horizontal(id="buttons"):
                yield Button(f"{self.yes} (y)", id="yes")
                yield Button("Cancel (n)", id="no")

    def on_mount(self) -> None:
        self.query_one("#no", Button).focus()

    def on_button_pressed(self, event: Button.Pressed) -> None:
        self.dismiss(event.button.id == "yes")

    def action_answer(self, yes: bool) -> None:
        self.dismiss(yes)


class FolderSuggester(Suggester):
    """Completes folder names as they're typed; → accepts."""

    def __init__(self):
        super().__init__(use_cache=False, case_sensitive=True)

    async def get_suggestion(self, value: str) -> str | None:
        if not value:
            return None
        head, tail = os.path.split(os.path.expanduser(value))
        try:
            names = sorted(n for n in os.listdir(head or "/") if n.startswith(tail)
                           and (tail.startswith(".") or not n.startswith("."))
                           and os.path.isdir(os.path.join(head or "/", n)))
        except OSError:
            return None
        return value + names[0][len(tail):] if names and names[0] != tail else None


class NewSessionScreen(ModalScreen["actions.NewSession | None"]):
    """Folder, name and an optional first prompt. Enter moves to the next field and starts the
    session from the last one; Esc cancels."""
    BINDINGS = [Binding("escape", "cancel", "Cancel", show=False)]

    def __init__(self, folder: str):
        super().__init__()
        self.folder = folder

    def compose(self) -> ComposeResult:
        with Vertical(id="dialog"):
            yield Label("New background session", classes="title")
            yield Label("Folder", classes="field")
            yield Input(short_path(self.folder), id="folder", suggester=FolderSuggester())
            yield Label("Name", classes="field")
            yield Input(placeholder="what this session is for", id="name")
            yield Label("First prompt (optional: without one it waits for you)", classes="field")
            yield Input(placeholder="what to do first", id="prompt")
            yield Static("", id="error")
            with Horizontal(id="buttons"):
                yield Button("Start (Enter)", id="start")
                yield Button("Cancel (Esc)", id="cancel")

    def on_mount(self) -> None:
        self.query_one("#name", Input).focus()

    def on_input_submitted(self, event: Input.Submitted) -> None:
        event.stop()
        nxt = {"folder": "#name", "name": "#prompt"}.get(event.input.id or "")
        if nxt:
            self.query_one(nxt, Input).focus()
        else:
            self.submit()

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "start":
            self.submit()
        else:
            self.dismiss(None)

    def submit(self) -> None:
        try:
            req = actions.plan_new(*(self.query_one(f"#{k}", Input).value for k in ("folder", "name", "prompt")))
        except actions.Refused as e:
            self.query_one("#error", Static).update(Text(str(e)))
            return
        self.dismiss(req)

    def action_cancel(self) -> None:
        self.dismiss(None)


class FilterInput(Input):
    """The filter box. Its keys show in the footer while it has the focus."""
    BINDINGS = [
        Binding("enter", "submit", "Back to the list", key_display="⏎"),
        Binding("escape", "app.clear_filter", "Clear filter", key_display="esc"),
    ]


# --- the board ---------------------------------------------------------------------------------

class Board(App):
    TITLE = "Claude sessions"
    CSS = """
    SessionTree { padding: 0 1; height: 1fr; }
    #filter { display: none; border: none; height: 1; padding: 0 1; background: $boost; }
    #filter.shown { display: block; }
    """ + DIALOG_CSS
    BINDINGS = [
        Binding("r", "refresh", "Refresh"),
        Binding("escape", "clear_filter", "Clear filter", show=False),
        Binding("q", "quit", "Quit"),
    ]

    def __init__(self, source=facts.collect, runner: Runner | None = None, poll: float | None = POLL_SECONDS,
                 show_ended: bool = False, prefs_path: str | None = None, show_columns: bool = True):
        super().__init__()
        # Ended sessions are hidden unless you show them with e; the filter always searches them.
        self.show_ended = show_ended
        self.show_columns = show_columns  # context, model and tokens on each row; c hides them
        self.prefs_path = prefs_path  # where the choice is remembered; None in tests
        self.source = source
        self.runner = runner or Runner()
        self.poll = poll
        self.sessions: dict[str, Session] = {}
        self.loaded: list[Session] = []
        # Rows you opened or closed yourself. A project with a live session is open unless you
        # closed it; everything else is open only if you opened it.
        self.open_nodes: set[tuple[str, str]] = set()
        self.closed_nodes: set[tuple[str, str]] = set()
        self.cursor_key: tuple[str, str] | None = None
        # A session just started: the cursor goes to it when it first shows up (for 30 s).
        self.pending_job: tuple[str, float] | None = None
        # The filter opens every project with a match, except ones closed while it is on.
        self.filter_terms: list[str] = []
        self.filter_closed: set[tuple[str, str]] = set()
        self._loaded = False

    def compose(self) -> ComposeResult:
        yield Header()
        tree = SessionTree("sessions", id="tree")
        tree.show_root = False
        tree.auto_expand = False
        tree.guide_depth = 3
        yield tree
        yield FilterInput(placeholder="filter by name, folder or project", id="filter")
        yield Footer()

    def notify(self, message: str, *, markup: bool = False, **kw) -> None:
        # Messages carry session titles and paths, which may hold [brackets]: plain text by default.
        super().notify(message, markup=markup, **kw)

    def check_action(self, action: str, parameters: tuple) -> bool | None:
        # A dialog is answered with its own keys; q must not quit the board from under it.
        if action in ("quit", "refresh", "clear_filter") and isinstance(self.screen, ModalScreen):
            return False
        return True

    def on_mount(self) -> None:
        self.sub_title = socket.gethostname()
        self.query_one(SessionTree).focus()
        self.reload()
        if self.poll:
            self.set_interval(self.poll, self.reload)

    # --- loading -------------------------------------------------------------------------------

    @work(thread=True, exclusive=True, group="load")
    def reload(self) -> None:
        sessions = self.source()
        self.call_from_thread(self.show, sessions)

    def show(self, sessions: list[Session]) -> None:
        tree = self.query_one(SessionTree)
        self.loaded = list(sessions)
        self.sessions = {s.session_id: s for s in sessions}
        counts = {st: sum(1 for s in sessions if s.state == st) for st in facts.STATES}
        filtering = bool(self.filter_terms)
        if filtering:
            sessions = [s for s in sessions if matches(s, self.filter_terms)]
        elif not self.show_ended:
            sessions = [s for s in sessions if s.state != "ended"]
        groups = facts.group(sessions)
        width = (tree.size.width or self.size.width or 100) - 4
        keep = self.cursor_key
        tree.clear()
        found = fresh = None
        if self.pending_job and time.time() - self.pending_job[1] > 30:
            self.pending_job = None
        for path, items in groups:
            pkey = ("project", path)
            pnode = tree.root.add(project_label(path, items), data=pkey)
            found = pnode if pkey == keep else found
            for s in items:
                skey = ("session", s.session_id)
                snode = pnode.add(session_label(s, width - 6, self.show_columns), data=skey)
                found = snode if skey == keep else found
                if self.pending_job and s.job == self.pending_job[0]:
                    fresh, self.pending_job = snode, None
                for line in detail_lines(s, width - 12):
                    leaf = snode.add_leaf(line, data=("detail", s.session_id))
                    if keep == ("detail", s.session_id) and found is None:
                        found = leaf
                if skey in self.open_nodes:
                    snode.expand()
            live = any(s.state != "ended" for s in items)
            if filtering:
                if pkey not in self.filter_closed:
                    pnode.expand()
            elif pkey in self.open_nodes or (live and pkey not in self.closed_nodes):
                pnode.expand()
        if filtering and not groups:
            tree.root.add_leaf(Text(f"No sessions match “{' '.join(self.filter_terms)}”.", style="dim italic"))
        sub = f"{socket.gethostname()} · {counts['needs you']} need you · {counts['running']} running"
        if counts["stale"]:
            sub += f" · {counts['stale']} not running"
        if filtering:
            sub += f" · filter: {' '.join(self.filter_terms)}"
        elif not self.show_ended and counts["ended"]:
            sub += f" · {counts['ended']} ended hidden (e)"
        self.sub_title = sub
        found = fresh or found
        if found is None and tree.root.children:
            found = tree.root.children[0]  # first load, or the remembered row is gone
        if found is not None:
            self.call_after_refresh(tree.move_cursor, found)
        self._loaded = True

    def redraw(self) -> None:
        """Show the last load again, for a filter change; the poll fetches fresh facts."""
        self.show(self.loaded)

    # --- remembering what is open ------------------------------------------------------------

    def on_tree_node_expanded(self, event: Tree.NodeExpanded) -> None:
        if not event.node.data:
            return
        if self.filter_terms and event.node.data[0] == "project":
            self.filter_closed.discard(event.node.data)
        else:
            self.open_nodes.add(event.node.data)
            self.closed_nodes.discard(event.node.data)

    def on_tree_node_collapsed(self, event: Tree.NodeCollapsed) -> None:
        if not event.node.data:
            return
        if self.filter_terms and event.node.data[0] == "project":
            self.filter_closed.add(event.node.data)
        else:
            self.open_nodes.discard(event.node.data)
            self.closed_nodes.add(event.node.data)

    def on_tree_node_highlighted(self, event: Tree.NodeHighlighted) -> None:
        if event.node.data:
            self.cursor_key = event.node.data

    def cursor_session(self) -> Session | None:
        node = self.query_one(SessionTree).cursor_node
        kind, key = (node.data if node and node.data else (None, None))
        return self.sessions.get(key) if kind in ("session", "detail") else None

    def refuse(self, e: actions.Refused) -> None:
        self.notify(str(e), severity="warning", timeout=8)

    # --- jumping in ----------------------------------------------------------------------------

    def on_tree_node_selected(self, event: Tree.NodeSelected) -> None:
        event.stop()
        kind, key = event.node.data or (None, None)
        if kind == "project":
            event.node.toggle()
        elif kind in ("session", "detail"):
            self.jump(key)

    def action_refresh(self) -> None:
        self.reload()

    def jump(self, session_id: str) -> None:
        s = self.sessions.get(session_id)
        if s is None:
            return
        try:
            target = actions.plan_enter(s)
            if isinstance(target, actions.ResumeThenAttach):
                self.notify(f"Resuming “{s.title}” in the background…")
                job = self.runner.resume(target.session_id, target.cwd)
            else:
                job = target.job
        except actions.Refused as e:
            self.refuse(e)
            return
        self.runner.attach(self, job)
        self.reload()

    # --- n: a new session ------------------------------------------------------------------------

    def default_folder(self) -> str:
        """The highlighted session's folder, else the project's. The scratch project's own folder is
        /tmp itself, so it offers its latest session's folder instead."""
        s = self.cursor_session()
        if s and os.path.isdir(s.cwd):
            return s.cwd
        node = self.query_one(SessionTree).cursor_node
        kind, key = (node.data if node and node.data else (None, None))
        if s:
            key = s.project
        elif kind != "project":
            return HOME
        if key == SCRATCH:
            for t in self.loaded:
                if t.project == SCRATCH and t.cwd != SCRATCH and os.path.isdir(t.cwd):
                    return t.cwd
        return key if key and os.path.isdir(key) else HOME

    def action_new_session(self) -> None:
        def chosen(req: actions.NewSession | None) -> None:
            if req is not None:
                self.call_after_refresh(self.start_new, req)
        self.push_screen(NewSessionScreen(self.default_folder()), chosen)

    def start_new(self, req: actions.NewSession) -> None:
        self.notify(f"Starting “{req.name or req.prompt[:50]}” in {short_path(req.cwd)}…")
        try:
            job = self.runner.new(req)
        except actions.Refused as e:
            self.refuse(e)
            self.reload()
            return
        self.pending_job = (job, time.time())
        self.runner.attach(self, job)
        self.reload()

    # --- s: stop -----------------------------------------------------------------------------------

    def action_stop(self) -> None:
        s = self.cursor_session()
        if s is None:
            self.notify("Highlight a session to stop it.", timeout=4)
            return
        try:
            job = actions.plan_stop(s)
        except actions.Refused as e:
            self.refuse(e)
            return
        if s.state != "running":
            self.stop(s, job)
            return
        self.push_screen(
            ConfirmScreen(f"“{s.title}” is working right now. Stop it?\n\n"
                          "Its conversation is kept; Enter opens it again.", "Stop"),
            lambda yes: self.stop(s, job) if yes else None)

    def stop(self, s: Session, job: str) -> None:
        try:
            self.runner.stop(job)
        except actions.Refused as e:
            self.refuse(e)
        else:
            self.notify(f"Stopped “{s.title}”.", timeout=4)
        self.reload()

    # --- x: delete -----------------------------------------------------------------------------------

    def action_delete(self) -> None:
        s = self.cursor_session()
        if s is None:
            self.notify("Highlight a session to delete it.", timeout=4)
            return
        try:
            job = actions.plan_delete(s)
        except actions.Refused as e:
            self.refuse(e)
            return
        running = " It's working right now; deleting stops it." if s.state == "running" else ""
        self.push_screen(
            ConfirmScreen(f"Delete “{s.title}”?{running}\n\n"
                          "This removes the background session, and its worktree if it has one. It "
                          "can't be undone. The board stops showing the conversation; Claude Code "
                          "keeps its transcript until it clears old ones (30 days).", "Delete"),
            lambda yes: self.delete(s, job) if yes else None)

    def delete(self, s: Session, job: str) -> None:
        try:
            self.runner.delete(s, job)
        except actions.Refused as e:
            self.notify(str(e), severity="error", timeout=15)
        else:
            self.notify(f"Deleted “{s.title}”.", timeout=4)
            self.cursor_key = self.neighbour(s.session_id)
        self.reload()

    def neighbour(self, session_id: str) -> tuple[str, str] | None:
        """Where the cursor goes when a session's row goes away: the next session in its project,
        else the one before, else the project."""
        tree = self.query_one(SessionTree)
        for project in tree.root.children:
            rows = [n for n in project.children if n.data and n.data[0] == "session"]
            keys = [n.data for n in rows]
            if ("session", session_id) in keys:
                i = keys.index(("session", session_id))
                others = keys[i + 1:] + keys[:i][::-1]
                return others[0] if others else project.data
        return None

    # --- /: filter -----------------------------------------------------------------------------------

    def action_toggle_ended(self) -> None:
        self.show_ended = not self.show_ended
        if self.prefs_path:
            facts.save_prefs({"show_ended": self.show_ended}, self.prefs_path)
        self.notify("Showing ended sessions." if self.show_ended else "Hiding ended sessions.", timeout=3)
        self.redraw()

    def action_toggle_columns(self) -> None:
        self.show_columns = not self.show_columns
        if self.prefs_path:
            facts.save_prefs({"show_columns": self.show_columns}, self.prefs_path)
        self.redraw()

    def action_filter(self) -> None:
        box = self.query_one("#filter", Input)
        box.add_class("shown")
        box.focus()

    def on_input_changed(self, event: Input.Changed) -> None:
        if event.input.id != "filter":
            return
        terms = event.value.lower().split()
        if terms != self.filter_terms:
            self.filter_terms = terms
            self.filter_closed.clear()
            self.redraw()

    def on_input_submitted(self, event: Input.Submitted) -> None:
        if event.input.id != "filter":
            return
        if not event.value.strip():
            event.input.remove_class("shown")
        self.query_one(SessionTree).focus()

    def action_clear_filter(self) -> None:
        box = self.query_one("#filter", Input)
        if not box.has_class("shown") and not self.filter_terms:
            return
        box.value = ""  # Input.Changed clears the terms and redraws
        box.remove_class("shown")
        self.query_one(SessionTree).focus()


# --- command line --------------------------------------------------------------------------------

def print_list(sessions: list[Session]) -> None:
    """`board --list`: the tree as plain text, for scripts and quick checks."""
    for path, items in facts.group(sessions):
        print(f"{project_name(path)}  ({short_path(path)})")
        for s in items:
            where = f"  [{s.job}]" if s.job else ""
            print(f"  {GLYPH[s.state]} {s.title[:60]}  — {state_word(s)}, {ago(s.last)}{where}")
            if s.waiting and s.state in ("needs you", "stale"):
                print(f"      {'needs' if s.state == 'needs you' else 'needed'}: {s.waiting[:100]}")


def as_json(sessions: list[Session]) -> list[dict]:
    """`board --json`: one object per row, for the Claude Code mod and other scripts. No transcript
    text, only what identifies a session and its state. "was" is what a stale job still claims."""
    return [{"session_id": s.session_id, "job": s.job, "title": s.title, "state": s.state,
             "stopped": s.stopped, "was": s.was, "project": s.project, "cwd": s.cwd,
             "last": round(s.last, 3), "model": s.model, "context": s.context, "output": s.output}
            for s in sessions]


HELP = """usage: board [--list | --json | --config]

  Projects, their sessions, and a summary of each. Enter jumps into a session;
  Ctrl+Z inside it (or ← then Esc) comes back here.

  Keys:  Enter  jump in (an ended session is resumed in the background first)
         → ←    open and close rows
         n      new background session (folder, name, optional first prompt), then jump in
         s      stop a background session; Enter opens it again
         x      delete a background session (asks first; can't be undone)
         /      filter by name, folder or project (ended ones too); Esc clears
         e      show or hide ended sessions (hidden at first; the choice is remembered)
         c      show or hide the context, model and token columns (remembered)
         r      refresh;  q  quit

  --list    print the tree as text and exit
  --config  print the settings in effect (workspaces, scratch folder, …) and
            where each came from: SESSION_BOARD_* variables, then
            ~/.config/session-board/config.toml, then defaults
  --json  print every session as JSON (session_id, job, title, state, stopped, was,
          project, cwd, last, model, context, output) and exit

  States: needs you · running · in another terminal · idle · not running (a
  background job whose state says it needs you or is working, but Claude Code
  has no process for it) · ended.  A row that needs you says what for."""


def main(argv: list[str] | None = None) -> int:
    argv = sys.argv[1:] if argv is None else argv
    if "-h" in argv or "--help" in argv:
        print(HELP)
        return 0
    if "--config" in argv:
        from . import config
        print(config.describe(facts.CONFIG))
        return 0
    if "--list" in argv:
        print_list(facts.collect())
        return 0
    if "--json" in argv:
        print(json.dumps(as_json(facts.collect())))
        return 0
    prefs = facts.load_prefs()
    Board(show_ended=bool(prefs.get("show_ended")), show_columns=prefs.get("show_columns", True) is not False,
          prefs_path=facts.PREFS_PATH).run()
    return 0
