"""Facts for the board: which sessions exist, what state each is in, and a summary.

Facts only, no model calls. Sources (see docs/data-sources.md):
- `claude agents --json --all`: background sessions and live interactive ones. If the CLI fails,
  the jobs folder and the live-process registry stand in for it.
- `~/.claude/jobs/<id>/state.json`: a background session's tempo and Claude Code's own summary.
- `~/.claude/daemon/roster.json`: the background workers that are running, to catch a job whose
  state.json still says "working" or "needs you" after its process went away.
- Transcripts under `~/.claude/projects`: titles, the first prompt, the last reply, a pending tool
  call, the model and token use, and ended sessions. Parsing them all takes about a second, so
  results are cached by file size and mtime.
"""
from __future__ import annotations

import glob
import json
import os
import re
import subprocess
import tempfile
import time
from dataclasses import dataclass
from datetime import datetime
from functools import lru_cache

from . import config

CONFIG = config.load()  # environment, then ~/.config/session-board/config.toml; see board/config.py
HOME = CONFIG.home
CLAUDE_DIR = CONFIG.claude_dir
CLAUDE = CONFIG.claude  # the claude CLI
CACHE_PATH = os.path.join(CONFIG.cache_dir, "transcripts.json")
# Conversations deleted from the board. `claude rm` deletes a background session but keeps its
# transcript, which would bring it straight back as an ended row; the board hides it instead.
DELETED_PATH = os.path.join(CONFIG.state_dir, "deleted.json")
DAYS = CONFIG.days  # Claude Code deletes transcripts after 30 days by default
# A repo or folder directly under one of these is a project of its own (~/work/api, /srv/app), even
# when the workspace itself is a git repo.
WORKSPACES = CONFIG.workspaces
# Sessions in throwaway folders under here are one project, "scratch".
SCRATCH = CONFIG.scratch
# A session resumed with `claude --bg --resume` and no prompt reports tempo "blocked" with this
# detail. It is waiting for a first prompt, not for Ryan, so it counts as idle.
IDLE_PLACEHOLDER = "(idle"

# Order matters: the board sorts by it. "stale": a background job whose state.json says it needs
# you or is working, while Claude Code has no process running it.
STATES = ("needs you", "running", "elsewhere", "idle", "stale", "ended")
RANK = {s: i for i, s in enumerate(STATES)}
CACHE_VERSION = 4


@dataclass
class Session:
    session_id: str
    title: str
    cwd: str
    project: str
    state: str                  # one of STATES
    last: float                 # epoch seconds of the last activity
    job: str | None = None      # short id for `claude attach`, for background sessions
    asked: str | None = None    # the first prompt Ryan typed
    summary: str | None = None  # Claude Code's own summary of a background session
    said: str | None = None     # the last thing Claude said, from the transcript
    stopped: bool = False       # a background session stopped with `claude stop`
    root: str | None = None     # uuid of the first message: the same for every copy of a conversation
    waiting: str | None = None  # what it needs from Ryan, or last needed when stale; secrets masked
    was: str | None = None      # for a stale session, the state its state.json still claims
    model: str | None = None    # the model of its last reply, e.g. "claude-opus-5-5"
    context: int = 0            # tokens in the context window at its last reply
    output: int = 0             # output tokens over the whole conversation


# --- transcripts -----------------------------------------------------------------------------

def _jsonl(path):
    with open(path, encoding="utf-8", errors="replace") as f:
        for line in f:
            try:
                yield json.loads(line)
            except ValueError:
                continue


def _epoch(stamp) -> float:
    try:
        return datetime.fromisoformat(str(stamp).replace("Z", "+00:00")).timestamp()
    except ValueError:
        return 0.0


def _text(content) -> str:
    if isinstance(content, str):
        return content
    return "\n".join(p.get("text", "") for p in content or []
                     if isinstance(p, dict) and p.get("type") == "text")


def parse_transcript(path: str) -> dict:
    """Read one transcript into the few fields the board uses."""
    t = {"cwd": None, "entry": None, "typed": 0, "asked": None, "ai": None, "custom": None,
         "agent": None, "last": 0.0, "said": None, "root": None, "cleared": False,
         "pending": None, "model": None, "context": 0, "output": 0}
    pending: dict[str, str] = {}  # tool calls with no result yet, by tool_use id
    outputs: dict[str, int] = {}  # one reply is written as several entries sharing its message id
    for e in _jsonl(path):
        kind = e.get("type")
        if not t["typed"] and _is_clear(e):
            # It began with /clear: a new conversation in an old session. Claude Code copies the
            # session's name into it as its title, so the title isn't its own.
            t["cleared"] = True
        if kind == "ai-title":
            t["ai"] = e.get("aiTitle")
        elif kind == "custom-title":
            t["custom"] = e.get("customTitle")
        elif kind == "agent-name":
            t["agent"] = e.get("agentName")
        elif kind in ("user", "assistant") and not e.get("isSidechain"):
            t["cwd"] = t["cwd"] or e.get("cwd")
            t["entry"] = t["entry"] or e.get("entrypoint")
            if kind == "user" and not t["root"]:
                # Copies of one conversation (a session moved to the background) keep the uuid of
                # its first message, so it identifies the conversation across copies.
                t["root"] = e.get("uuid")
            t["last"] = max(t["last"], _epoch(e.get("timestamp")))
            msg = e.get("message") or {}
            text = _text(msg.get("content")).strip()
            if kind == "user":
                for part in _parts(msg.get("content")):
                    if part.get("type") == "tool_result":
                        pending.pop(part.get("tool_use_id"), None)
            if kind == "user" and e.get("promptSource") in ("typed", "queued"):
                pending.clear()  # a new prompt: whatever was asked before isn't waiting any more
                if text and not text.startswith("/"):
                    t["typed"] += 1
                    t["asked"] = t["asked"] or text
            elif kind == "assistant":
                if text:
                    t["said"] = text
                for part in _parts(msg.get("content")):
                    if part.get("type") == "tool_use":
                        pending[part.get("id")] = describe_tool(part.get("name"), part.get("input"))
                usage = msg.get("usage") or {}
                if msg.get("model") and msg.get("model") != "<synthetic>" and usage:
                    t["model"] = msg["model"]
                    t["context"] = sum(int(usage.get(k) or 0) for k in (
                        "input_tokens", "cache_creation_input_tokens", "cache_read_input_tokens"))
                    key = msg.get("id") or e.get("uuid")
                    outputs[key] = max(outputs.get(key, 0), int(usage.get("output_tokens") or 0))
    t["output"] = sum(outputs.values())
    t["pending"] = list(pending.values())[-1] if pending else None
    for key in ("asked", "said"):
        if t[key] and len(t[key]) > 1200:
            t[key] = t[key][:1200]
    return t


def _parts(content) -> list[dict]:
    return [p for p in content if isinstance(p, dict)] if isinstance(content, list) else []


def describe_tool(name, args) -> str:
    """A pending tool call in a few words: the question asked, or the tool and what it acts on."""
    args = args if isinstance(args, dict) else {}
    if name == "AskUserQuestion":
        qs = args.get("questions") or []
        q = qs[0].get("question") if qs and isinstance(qs[0], dict) else None
        if q:
            return mask_secrets(_flat(q))
    main = (args.get("command") or args.get("file_path") or args.get("path") or args.get("url")
            or args.get("pattern") or args.get("description") or args.get("prompt"))
    if not isinstance(main, str):
        main = next((v for v in args.values() if isinstance(v, str)), "")
    return mask_secrets(f"{name}: {_flat(main)}" if main else str(name or "a tool"))[:300]


def _flat(text: str) -> str:
    return " ".join(str(text).split())


# Values that look like credentials: NAME=value where the name says key/token/secret/password,
# bearer and basic auth, URLs with a password, and long opaque strings.
_SECRET_PATTERNS = [
    (re.compile(r"(?i)\b(bearer|basic|token)\s+[A-Za-z0-9._~+/=-]{8,}"), r"\1 …"),
    (re.compile(r"(?i)\b([A-Z0-9_.-]*(?:KEY|TOKEN|SECRET|PASSWORD|PASSWD|PASS|PWD|AUTH|CREDENTIALS?)[A-Z0-9_.-]*)(\s*[=:]\s*)(\"[^\"]*\"|'[^']*'|\S+)"),
     r"\1\2…"),
    (re.compile(r"(?i)(--?(?:password|passwd|token|api-key|apikey|secret|key)[= ])\S+"), r"\1…"),
    (re.compile(r"(://[^/\s:@]+:)[^@\s/]+@"), r"\1…@"),
    (re.compile(r"[A-Za-z0-9_+/=-]{32,}"), "…"),
]


def mask_secrets(text: str | None) -> str | None:
    """Masks what looks like a credential. Commands go on screen and into the cache, never raw."""
    if not text:
        return text
    for pattern, repl in _SECRET_PATTERNS:
        text = pattern.sub(repl, text)
    return text


def _is_clear(e: dict) -> bool:
    if e.get("type") == "attachment":
        return (e.get("attachment") or {}).get("hookName") == "SessionStart:clear"
    if e.get("type") == "system":
        return (e.get("commandRun") or {}).get("command") == "clear"
    return False


def own_title(t: dict) -> str | None:
    """The title a transcript gave itself; None for one that began with /clear (an inherited name)."""
    return None if t.get("cleared") else (t.get("custom") or t.get("agent") or t.get("ai"))


def _ended_title(t: dict, before_clear: bool) -> str:
    if before_clear:
        # The job's name most likely came from this conversation, its first after the job began.
        name = t.get("custom") or t.get("agent") or t.get("ai") or _first_line(t.get("asked"))
        return f"{name} (before /clear)"
    return own_title(t) or _first_line(t.get("asked"))


def load_transcripts(claude_dir: str = CLAUDE_DIR, cache_path: str = CACHE_PATH,
                     now: float | None = None) -> dict[str, dict]:
    """Parsed transcripts from the last DAYS days, keyed by session id."""
    now = time.time() if now is None else now
    try:
        with open(cache_path) as f:
            cache = json.load(f)
        if cache.get("version") != CACHE_VERSION:
            cache = {}
    except (OSError, ValueError):
        cache = {}
    old = cache.get("files", {})
    files, changed = {}, False
    for path in glob.glob(os.path.join(claude_dir, "projects", "*", "*.jsonl")):
        try:
            st = os.stat(path)
        except OSError:
            continue
        if st.st_mtime < now - DAYS * 86400:
            continue
        sig = [st.st_mtime_ns, st.st_size]
        hit = old.get(path)
        if hit and hit.get("sig") == sig:
            files[path] = hit
        else:
            files[path] = {"sig": sig, "info": parse_transcript(path)}
            changed = True
    if changed or set(files) != set(old):
        _write_private(cache_path, {"version": CACHE_VERSION, "files": files})
    return {os.path.basename(p)[:-len(".jsonl")]: v["info"] for p, v in files.items()}


def _write_private(path: str, data) -> None:
    """Write atomically, readable by Ryan only: the cache holds transcript excerpts."""
    os.makedirs(os.path.dirname(path), mode=0o700, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=os.path.dirname(path), prefix=".tmp-")
    try:
        with os.fdopen(fd, "w") as f:
            json.dump(data, f)
        os.replace(tmp, path)
    except BaseException:
        os.unlink(tmp)
        raise


# --- the board's remembered choices --------------------------------------------------------------

PREFS_PATH = os.path.join(os.path.dirname(DELETED_PATH), "prefs.json")


def load_prefs(path: str = PREFS_PATH) -> dict:
    data = _read_json(path)
    return data if isinstance(data, dict) else {}


def save_prefs(prefs: dict, path: str = PREFS_PATH) -> None:
    try:
        _write_private(path, {**load_prefs(path), **prefs})
    except OSError:
        pass  # a choice not remembered is no reason to stop


# --- conversations deleted from the board -------------------------------------------------------

def load_deleted(path: str = DELETED_PATH) -> dict[str, float]:
    """Session ids and conversation roots deleted from the board, with when."""
    data = _read_json(path)
    if not isinstance(data, dict):
        return {}
    return {k: float(v) for k, v in data.items() if isinstance(v, (int, float))}


def forget(session_id: str, root: str | None, path: str = DELETED_PATH, now: float | None = None) -> None:
    """Hide a deleted session's conversation, every copy of it, from the ended rows. Entries go
    after DAYS, when Claude Code has cleared the transcripts anyway."""
    now = time.time() if now is None else now
    data = {k: v for k, v in load_deleted(path).items() if v > now - (DAYS + 1) * 86400}
    for key in (session_id, root):
        if key:
            data[key] = now
    _write_private(path, data)


def _deleted(deleted: dict[str, float], session_id: str, root: str | None, last: float) -> bool:
    """Hidden, unless the conversation was used again after it was deleted."""
    at = max(deleted.get(session_id, 0.0), deleted.get(root, 0.0) if root else 0.0)
    return at > 0 and last <= at


# --- live sessions -------------------------------------------------------------------------------

def start_agents_cli():
    """Start `claude agents --json --all` so it runs while transcripts are parsed."""
    try:
        return subprocess.Popen([*claude_cmd(), "agents", "--json", "--all"], stdout=subprocess.PIPE,
                                stderr=subprocess.DEVNULL, text=True)
    except OSError:
        return None


def claude_cmd() -> list[str]:
    import shlex
    return shlex.split(CLAUDE) or ["claude"]


def finish_agents_cli(proc) -> list[dict] | None:
    if proc is None:
        return None
    try:
        out, _ = proc.communicate(timeout=10)
        data = json.loads(out)
        return data if proc.returncode == 0 and isinstance(data, list) else None
    except (subprocess.TimeoutExpired, ValueError):
        proc.kill()
        return None


def agents_from_files(claude_dir: str = CLAUDE_DIR) -> list[dict]:
    """What `claude agents --json --all` reports, rebuilt from files, for when the CLI fails."""
    out = []
    for path in glob.glob(os.path.join(claude_dir, "jobs", "*", "state.json")):
        st = _read_json(path)
        if st:
            out.append({"id": os.path.basename(os.path.dirname(path)), "kind": "background",
                        "sessionId": st.get("sessionId"), "cwd": st.get("cwd"), "name": st.get("name")})
    for path in glob.glob(os.path.join(claude_dir, "sessions", "*.json")):
        r = _read_json(path)
        if r and r.get("kind") == "interactive" and _alive(r.get("pid"), r.get("procStart")):
            out.append({"kind": "interactive", "sessionId": r.get("sessionId"), "cwd": r.get("cwd"),
                        "name": r.get("name"), "pid": r.get("pid")})
    return out


def _alive(pid, proc_start) -> bool:
    """A registry file outlives a crash, so the pid must exist and, on Linux, its start time must
    match (a reused pid would not). Without /proc (macOS) the pid existing has to do."""
    try:
        pid = int(pid)
    except (ValueError, TypeError):
        return False
    if pid <= 0:
        return False
    if not os.path.isdir("/proc/self"):
        try:
            os.kill(pid, 0)
            return True
        except PermissionError:
            return True  # it exists, owned by someone else
        except OSError:
            return False
    try:
        with open(f"/proc/{pid}/stat") as f:
            return f.read().rsplit(")", 1)[1].split()[19] == str(proc_start)
    except (OSError, IndexError):
        return False


def _read_json(path):
    try:
        with open(path) as f:
            return json.load(f)
    except (OSError, ValueError):
        return None


def live_workers(claude_dir: str = CLAUDE_DIR) -> set[str] | None:
    """The background jobs Claude Code has a process for, or None when that can't be told (no
    roster, as before the background service first ran). The supervisor restarts a worker that
    dies, but after the supervisor itself goes (a reboot, a crash) no job runs until it is opened."""
    roster = _read_json(os.path.join(claude_dir, "daemon", "roster.json"))
    if not isinstance(roster, dict) or not isinstance(roster.get("workers"), dict):
        return None
    return {job for job, w in roster["workers"].items()
            if isinstance(w, dict) and _alive(w.get("pid"), w.get("procStart"))}


def job_state(st: dict) -> str:
    """A background session's board state. Status lives in `tempo`, not `state`: a job mid-turn
    reports state "blocked" with tempo "active"."""
    tempo, detail = st.get("tempo"), st.get("detail") or ""
    if (tempo == "blocked" or st.get("state") == "needs_input") and not detail.startswith(IDLE_PLACEHOLDER):
        return "needs you"
    if tempo == "active":
        return "running"
    return "idle"


# --- projects ------------------------------------------------------------------------------------

@lru_cache(maxsize=512)
def git_root(path: str) -> str | None:
    d = path
    while d and d != "/":
        if os.path.exists(os.path.join(d, ".git")):
            return d
        d = os.path.dirname(d)
    return None


def project_of(cwd: str | None, workspaces=None, scratch: str | None = None) -> str:
    workspaces = WORKSPACES if workspaces is None else workspaces
    scratch = SCRATCH if scratch is None else scratch
    if not cwd:
        return "?"
    if cwd == scratch or cwd.startswith(scratch + "/"):
        return scratch
    root = git_root(cwd)
    if root and root not in workspaces:
        return root
    for w in sorted(workspaces, key=len, reverse=True):
        if cwd == w:
            return w
        if cwd.startswith(w + "/"):
            return os.path.join(w, cwd[len(w) + 1:].split("/")[0])
    return root or cwd


# --- putting it together -----------------------------------------------------------------------

def collect(claude_dir: str = CLAUDE_DIR, cache_path: str = CACHE_PATH, agents=None,
            now: float | None = None, deleted_path: str = DELETED_PATH) -> list[Session]:
    """Every session the board shows. `agents` stands in for the CLI's output in tests."""
    proc = start_agents_cli() if agents is None else None
    transcripts = load_transcripts(claude_dir, cache_path, now)
    deleted = load_deleted(deleted_path)
    if agents is None:
        agents = finish_agents_cli(proc)
        if agents is None:
            agents = agents_from_files(claude_dir)

    workers = live_workers(claude_dir)
    out: dict[str, Session] = {}
    resumed = set()  # transcripts a background session continues from
    before_clear = set()  # a background session's conversation from before a /clear
    interactive = {a.get("sessionId") for a in agents if a.get("kind") == "interactive"}
    for a in agents:
        if a.get("kind") != "background" or not a.get("sessionId") or not a.get("id"):
            continue
        sid = a["sessionId"]
        st = _read_json(os.path.join(claude_dir, "jobs", a["id"], "state.json")) or {}
        resumed.add(st.get("resumeSessionId"))
        current = st.get("resumeSessionId")
        if current and current != sid and transcripts.get(current, {}).get("cleared"):
            # After a /clear the job keeps its first session id; resumeSessionId names the
            # conversation it is on now, and the first one is an ended conversation of its own.
            before_clear.add(sid)
            sid = current
        if sid in interactive:
            continue  # the conversation went on in a terminal (`claude --continue`); that row wins
        t = transcripts.get(sid, {})
        detail = st.get("detail") or ""
        summary = (st.get("output") or {}).get("result") or (
            detail if detail and not detail.startswith(IDLE_PLACEHOLDER) else None)
        cwd = a.get("cwd") or st.get("cwd") or t.get("cwd") or HOME
        state, was, stopped = job_state(st), None, st.get("state") == "stopped"
        if state in ("needs you", "running") and not stopped and workers is not None and a["id"] not in workers:
            state, was = "stale", state
        waiting = None
        if (was or state) == "needs you":
            waiting = mask_secrets(_flat(st.get("needs") or "")) or t.get("pending")
        out[sid] = Session(
            session_id=sid, job=a["id"], cwd=cwd, project=project_of(cwd), state=state,
            title=a.get("name") or st.get("name") or t.get("custom") or t.get("ai") or _first_line(t.get("asked")),
            last=max(t.get("last", 0.0), _epoch(st.get("updatedAt"))),
            asked=t.get("asked"), summary=summary, said=t.get("said"),
            stopped=stopped, root=t.get("root"), waiting=waiting, was=was, **_usage(t))

    for a in agents:
        sid = a.get("sessionId")
        if a.get("kind") != "interactive" or not sid or sid in out:
            continue
        t = transcripts.get(sid)
        if not t or not t["typed"]:
            continue  # no prompt yet
        cwd = a.get("cwd") or t.get("cwd") or HOME
        out[sid] = Session(
            session_id=sid, cwd=cwd, project=project_of(cwd), state="elsewhere",
            title=(a.get("name") if t.get("cleared") else None) or own_title(t) or a.get("name")
            or _first_line(t.get("asked")),
            last=t["last"], asked=t.get("asked"), said=t.get("said"), root=t.get("root"), **_usage(t))

    live_roots = {transcripts.get(sid, {}).get("root") for sid in out} - {None}
    ended_by_root: dict[str, str] = {}
    for sid, t in sorted(transcripts.items(), key=lambda kv: -kv[1].get("last", 0.0)):
        if sid in out or t.get("entry") != "cli" or not t.get("typed"):
            continue  # automation (plugins, headless runs), or opened and closed without a prompt
        root = t.get("root")
        if sid in resumed or root in live_roots or (root and root in ended_by_root):
            continue  # an older copy of a conversation already on the board
        if _deleted(deleted, sid, root, t["last"]):
            continue  # deleted from the board; `claude rm` keeps the transcript
        if root:
            ended_by_root[root] = sid
        cwd = t.get("cwd") or HOME
        out[sid] = Session(
            session_id=sid, cwd=cwd, project=project_of(cwd), state="ended",
            title=_ended_title(t, sid in before_clear),
            last=t["last"], asked=t.get("asked"), said=t.get("said"), root=root, **_usage(t))

    return sorted(out.values(), key=lambda s: (RANK[s.state], -s.last))


def _usage(t: dict) -> dict:
    return {"model": t.get("model"), "context": t.get("context") or 0, "output": t.get("output") or 0}


def _first_line(text: str | None) -> str:
    return (text or "").strip().splitlines()[0][:80] if (text or "").strip() else "(untitled)"


def group(sessions: list[Session]) -> list[tuple[str, list[Session]]]:
    """Sessions by project, the project with the most urgent session first, then the most recent."""
    by: dict[str, list[Session]] = {}
    for s in sessions:
        by.setdefault(s.project, []).append(s)
    for items in by.values():
        items.sort(key=lambda s: (RANK[s.state], -s.last))
    return sorted(by.items(), key=lambda kv: (min(RANK[s.state] for s in kv[1]),
                                             -max(s.last for s in kv[1])))
