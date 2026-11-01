# session-board

A terminal board for every Claude Code session on a machine, as a replacement for Claude Code's Agent
View. It shows background sessions (`claude --bg`), sessions open in other terminals, and the last 30
days of ended conversations, grouped by project, with what each one is doing and what it needs from you.
Enter jumps into a session (`claude attach`); **Ctrl+Z** inside it comes back to the board.

```
▼ api  ~/work/api    1 needs you  ·  1 idle  ·  5m
├─ ◆ Fix the flaky auth test — Reply yes to commit the fix          needs you · 5m     182k ctx  Opus 5.5     31k out
└─ ■ Upgrade the ORM                                                idle · 2d           64k ctx  Sonnet 4.5   12k out
▼ infra  ~/work/infra    1 running  ·  1 not running  ·  now
├─ ● Terraform plan for the new queue                               running · now       22k ctx  Opus 5.5      4k out
└─ ◇ Rotate the staging certs — rate limited, wait and retry        not running · 3d    90k ctx  Opus 5.5     20k out
```

It reads Claude Code's own files and its CLI, and every action goes through the `claude` CLI. Nothing
leaves the machine.

## Requirements

- Claude Code with background sessions (`claude --bg`, `claude attach`, `claude agents`). Checked on
  2.1.316. The board reads files Claude Code doesn't document, so a Claude Code update can break it;
  `docs/data-sources.md` lists what it reads.
- Python 3.12+ and [uv](https://docs.astral.sh/uv/).
- Linux or macOS. On Linux, liveness checks also compare process start times, which macOS can't.

## Install

```bash
git clone git@github.com:ryanckelly/session-board.git ~/session-board
cd ~/session-board
uv sync                                   # creates .venv with Textual
ln -s ~/session-board/bin/board ~/.local/bin/board
ln -s ~/session-board/bin/cs ~/.local/bin/cs        # optional, see "cs" below
board                                     # or: board --list, board --json, board --help
```

## Keys

| Key | Does |
|---|---|
| Enter | Jump into the session. An ended one is resumed in the background first. |
| Ctrl+Z (inside a session) | Back to the board. ← then Esc also works, through Agent View. |
| → ← | Open and close rows. An open session shows what was asked, its status, model and tokens. |
| `n` | New background session: folder, optional name, optional first prompt. |
| `s` / `x` | Stop / delete a background session. Delete asks first. |
| `/` | Filter by name, folder or project, including ended sessions. Esc clears. |
| `e` | Show or hide ended sessions. Hidden at first; remembered. |
| `c` | Show or hide the context, model and token columns. Remembered. |
| `r` / `q` | Refresh / quit. |

## States

| | State | Meaning |
|---|---|---|
| ◆ | needs you | A background session is waiting for you. The row says what for, from Claude Code's own words or the pending tool call. |
| ● | running | Working on a turn. |
| ● | in another terminal | An interactive session open elsewhere. It can't be entered from here. |
| ■ | idle | Waiting for a prompt, or stopped. |
| ◇ | not running | A background job that claims to need you or to be working, but Claude Code has no process for it: its state is out of date (after a reboot, say). Enter starts it again. |
| ○ | ended | A saved conversation. Enter resumes it in the background. |

## Configuration

Defaults work without any config. To change them, create `~/.config/session-board/config.toml`:

```toml
# A folder directly under a workspace is a project of its own (~/work/api, ~/work/infra), even when
# the workspace itself is a git repo. Sessions elsewhere are grouped by their git repo.
workspaces = ["~", "~/work"]
scratch = "/tmp"        # throwaway sessions under here are one "scratch" project
days = 30               # how far back ended sessions go; match Claude Code's cleanupPeriodDays
poll = 10               # seconds between refreshes
claude = "claude"       # the claude CLI, if it isn't on PATH as "claude"
```

Each setting can also come from an environment variable, which wins over the file:
`SESSION_BOARD_WORKSPACES` (folders separated by `:`), `SESSION_BOARD_SCRATCH`, `SESSION_BOARD_DAYS`,
`SESSION_BOARD_POLL` and `SESSION_BOARD_CLAUDE`. `SESSION_BOARD_CONFIG` points at another config file.
Claude Code's folder follows `CLAUDE_CONFIG_DIR`, as Claude Code does. The cache and the board's own
state follow `XDG_CACHE_HOME` and `XDG_STATE_HOME`.

`board --config` prints the settings in effect and where each came from.

## The mod: "N other sessions need you" inside Claude Code

`mod/` is a Claude Code mod. It draws a band above the prompt in every session when other sessions need
you, e.g. "◆ 2 other sessions need you: Fix the flaky auth test · Rotate the staging certs ·
Ctrl+Z: board". It reads `board --json` every 30 seconds.

To load it in every session, add this to the `env` block of `~/.claude/settings.json`, using the path
of your clone:

```json
"CLAUDE_CODE_PLUGIN_DIRS": "/path/to/session-board/mod"
```

It runs `~/.local/bin/board`. If the board is installed elsewhere, also set `"SESSION_BOARD_BIN":
"/path/to/board"` in the same block. To try it in a single session: `claude --plugin-dir
/path/to/session-board/mod`. Check it with `claude plugin validate mod/` and `claude plugin test mod/`.

What a mod can't do (Claude Code 2.1.316): redirect ← to this board instead of Agent View, or end
`claude attach`. Ctrl+Z is the direct way back.

## cs: a numbered menu

`bin/cs` lists the same sessions as a numbered menu: type a number to jump in. It suits a phone SSH app
(as a host's startup command) where a full-screen board is awkward. `cs <text>` filters; `cs --list`
prints the menu.

## Files the board writes

- `~/.cache/session-board/transcripts.json`: parsed transcripts, cached by size and mtime, readable by
  you only. It holds excerpts (first prompt, last reply), with anything that looks like a credential in a
  pending command masked. Safe to delete.
- `~/.local/state/session-board/prefs.json`: the `e` and `c` choices.
- `~/.local/state/session-board/deleted.json`: conversations deleted with `x`. `claude rm` deletes the
  background session but keeps its transcript, which would otherwise come back as an ended row. Entries
  expire after the `days` setting.

## Development

```bash
.venv/bin/python -m pytest          # the board
claude plugin test mod/             # the mod
```

`board/facts.py` turns Claude Code's files into sessions and states; `board/app.py` is the Textual app;
`board/actions.py` wraps the `claude` CLI; `board/config.py` reads the settings. The tests never read the
machine's own config or Claude Code folder.
