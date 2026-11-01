"""What the board does to sessions, always through the claude CLI, never by editing ~/.claude.

Getting in: attach to a background session, or resume an ended one in the background first. Tested
2026-11-01 (docs/CONTEXT.md, "Pivot to a terminal board"): inside an attached session, Ctrl+Z ends
`claude attach` with code 0, and so does ← then Esc; the session keeps running either way. So the
board suspends itself, runs `claude attach`, and takes the terminal back when it exits.

New, stop, delete (checked on Claude Code 2.1.316):
- `claude --bg [--name N] [-- prompt]` prints `backgrounded · <id> · <name>`. With no prompt the
  session runs no turn and waits, like a resumed one.
- `claude stop <id>` keeps the conversation; the job reports state "stopped" and `claude attach`
  opens it again. Stopping twice also exits 0.
- `claude rm <id>` deletes the job (and its worktree when safe) but keeps the transcript. A second
  `rm` fails: "No job matching '<id>'".
"""
from __future__ import annotations

import os
import re
import subprocess
from dataclasses import dataclass

from . import facts
from .facts import HOME, Session


class Refused(Exception):
    """The session can't be entered from here; the message says why, in plain words."""


@dataclass(frozen=True)
class Attach:
    job: str


@dataclass(frozen=True)
class ResumeThenAttach:
    session_id: str
    cwd: str


@dataclass(frozen=True)
class NewSession:
    cwd: str
    name: str = ""
    prompt: str = ""


def plan_enter(s: Session) -> Attach | ResumeThenAttach:
    if s.state == "elsewhere":
        raise Refused(f"“{s.title}” is open in another terminal. Leave it there, or send it to the "
                      "background with /bg, and it can be entered from here.")
    if s.job:
        return Attach(s.job)
    if not os.path.isdir(s.cwd):
        raise Refused(f"“{s.title}” can't be resumed: its folder {s.cwd} no longer exists.")
    return ResumeThenAttach(s.session_id, s.cwd)


def plan_new(folder: str, name: str = "", prompt: str = "") -> NewSession:
    """Check what was typed in the new-session form."""
    folder = os.path.abspath(os.path.expanduser(folder.strip())) if folder.strip() else ""
    if not folder:
        raise Refused("Say which folder the session should start in.")
    if not os.path.isdir(folder):
        raise Refused(f"There's no folder {folder}.")
    name, prompt = " ".join(name.split()), prompt.strip()
    if not name and not prompt:
        raise Refused("Give the session a name or a first prompt.")
    return NewSession(folder, name, prompt)


def plan_stop(s: Session) -> str:
    """The job to stop, or Refused."""
    if s.state == "elsewhere":
        raise Refused(f"“{s.title}” is open in another terminal; stop it there.")
    if not s.job:
        raise Refused(f"“{s.title}” has already ended; there's nothing to stop.")
    if s.stopped:
        raise Refused(f"“{s.title}” is already stopped. Enter opens it again.")
    return s.job


def plan_delete(s: Session) -> str:
    """The job to delete, or Refused."""
    if s.state == "elsewhere":
        raise Refused(f"“{s.title}” is open in another terminal. Only background sessions can be "
                      "deleted from here.")
    if not s.job:
        raise Refused(f"“{s.title}” has ended and is only a saved conversation. Claude Code can't "
                      "delete one conversation on its own (only a whole project's data), so it stays "
                      f"until Claude Code clears it after {facts.DAYS} days.")
    return s.job


_BACKGROUNDED = re.compile(r"backgrounded · ([0-9a-f]{6,})")


def _claude(argv: list[str], cwd: str, failed: str, run=subprocess.run) -> str:
    """Run a claude CLI command that finishes on its own; return its output or raise Refused."""
    try:
        r = run([*facts.claude_cmd(), *argv], cwd=cwd, capture_output=True, text=True, timeout=60)
    except (OSError, subprocess.TimeoutExpired) as e:
        raise Refused(f"{failed}: {e}") from e
    if r.returncode != 0:
        raise Refused(f"{failed}: " + ((r.stderr or r.stdout).strip() or f"exit code {r.returncode}"))
    return r.stdout


def resume_in_background(session_id: str, cwd: str, run=subprocess.run) -> str:
    """Bring an ended session back as an idle background session and return its short id.
    It runs no turn: `claude --bg --resume` without a prompt waits for one."""
    out = _claude(["--bg", "--resume", session_id], cwd, "Couldn't resume the session", run)
    m = _BACKGROUNDED.search(out)
    return m.group(1) if m else session_id[:8]


def new_argv(req: NewSession) -> list[str]:
    argv = ["--bg"]
    if req.name:
        argv += ["--name", req.name]
    if req.prompt:
        argv += ["--", req.prompt]  # `--` so a prompt starting with "-" isn't read as an option
    return argv


def start_in_background(req: NewSession, run=subprocess.run) -> str:
    """Start a background session and return its short id."""
    out = _claude(new_argv(req), req.cwd, "Couldn't start the session", run)
    m = _BACKGROUNDED.search(out)
    if not m:
        raise Refused("Started a session but couldn't read its id from: " + (out.strip() or "(no output)"))
    return m.group(1)


def stop_background(job: str, run=subprocess.run) -> None:
    _claude(["stop", job], HOME, "Couldn't stop the session", run)


def delete_background(job: str, run=subprocess.run) -> None:
    _claude(["rm", job], HOME, "Couldn't delete the session", run)


def attach_argv(job: str) -> tuple[list[str], str]:
    return [*facts.claude_cmd(), "attach", job], HOME
