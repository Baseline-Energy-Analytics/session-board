"""The board's settings: environment variables, then a config file, then defaults.

    ~/.config/session-board/config.toml   (or $SESSION_BOARD_CONFIG, or $XDG_CONFIG_HOME/…)

    workspaces = ["~", "/opt"]   # a folder directly under one of these is a project of its own
    scratch    = "/tmp"          # throwaway sessions here are grouped as one "scratch" project
    days       = 30              # how far back ended sessions go (Claude Code's cleanupPeriodDays)
    poll       = 10              # seconds between refreshes
    claude     = "claude"        # the claude CLI

Every setting has an environment variable that wins over the file: SESSION_BOARD_WORKSPACES
(separated by ":"), SESSION_BOARD_SCRATCH, SESSION_BOARD_DAYS, SESSION_BOARD_POLL,
SESSION_BOARD_CLAUDE. Claude Code's own folder comes from CLAUDE_CONFIG_DIR, as in Claude Code.
The cache and the board's own state follow XDG_CACHE_HOME and XDG_STATE_HOME.
`board --config` prints what is in effect and where each value came from.
"""
from __future__ import annotations

import os
import tempfile
import tomllib
from dataclasses import dataclass, field

ENV_PREFIX = "SESSION_BOARD_"


@dataclass(frozen=True)
class Config:
    home: str
    claude_dir: str
    workspaces: tuple[str, ...]
    scratch: str
    days: int
    poll: float
    claude: str
    cache_dir: str
    state_dir: str
    config_path: str
    source: dict[str, str] = field(default_factory=dict, compare=False)  # setting → where it came from


def _expand(path: str, home: str) -> str:
    path = os.path.expandvars(str(path).strip())
    if path == "~" or path.startswith("~/"):
        path = home + path[1:]
    return os.path.normpath(os.path.abspath(path))


def load(env: dict[str, str] | None = None) -> Config:
    env = dict(os.environ) if env is None else env
    home = env.get("HOME") or os.path.expanduser("~")
    xdg_config = env.get("XDG_CONFIG_HOME") or os.path.join(home, ".config")
    config_path = _expand(env.get(ENV_PREFIX + "CONFIG") or os.path.join(xdg_config, "session-board", "config.toml"), home)
    try:
        with open(config_path, "rb") as f:
            file = tomllib.load(f)
    except FileNotFoundError:
        file = {}
    except (OSError, tomllib.TOMLDecodeError) as e:
        raise SystemExit(f"session-board: can't read {config_path}: {e}")

    source: dict[str, str] = {}

    def pick(name: str, default):
        var = ENV_PREFIX + name.upper()
        if env.get(var):
            source[name] = f"${var}"
            return env[var]
        if name in file:
            source[name] = config_path
            return file[name]
        source[name] = "default"
        return default

    raw_ws = pick("workspaces", ["~"])
    if isinstance(raw_ws, str):
        raw_ws = [p for p in raw_ws.split(os.pathsep) if p.strip()]
    workspaces = tuple(dict.fromkeys(_expand(p, home) for p in raw_ws))

    def number(name: str, default, kind):
        value = pick(name, default)
        try:
            n = kind(value)
            if n <= 0:
                raise ValueError
            return n
        except (TypeError, ValueError):
            raise SystemExit(f"session-board: {name} must be a positive number, not {value!r} "
                             f"(from {source[name]})")

    claude_dir = env.get("CLAUDE_CONFIG_DIR")
    source["claude_dir"] = "$CLAUDE_CONFIG_DIR" if claude_dir else "default"
    return Config(
        home=home,
        claude_dir=_expand(claude_dir or os.path.join(home, ".claude"), home),
        workspaces=workspaces,
        scratch=_expand(pick("scratch", tempfile.gettempdir()), home),
        days=number("days", 30, int),
        poll=number("poll", 10, float),
        claude=str(pick("claude", "claude")),
        cache_dir=os.path.join(env.get("XDG_CACHE_HOME") or os.path.join(home, ".cache"), "session-board"),
        state_dir=os.path.join(env.get("XDG_STATE_HOME") or os.path.join(home, ".local", "state"), "session-board"),
        config_path=config_path,
        source=source,
    )


def describe(c: Config) -> str:
    """`board --config`: the settings in effect, and where each came from."""
    rows = [
        ("workspaces", " ".join(c.workspaces), c.source.get("workspaces")),
        ("scratch", c.scratch, c.source.get("scratch")),
        ("days", str(c.days), c.source.get("days")),
        ("poll", f"{c.poll:g}", c.source.get("poll")),
        ("claude", c.claude, c.source.get("claude")),
        ("claude_dir", c.claude_dir, c.source.get("claude_dir")),
        ("cache_dir", c.cache_dir, "$XDG_CACHE_HOME or ~/.cache"),
        ("state_dir", c.state_dir, "$XDG_STATE_HOME or ~/.local/state"),
    ]
    found = "" if os.path.exists(c.config_path) else "  (not found; defaults apply)"
    out = [f"config file: {c.config_path}{found}"]
    out += [f"  {k:<11} {v}    [{s}]" for k, v, s in rows]
    return "\n".join(out)
