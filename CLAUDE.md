# CLAUDE.md

session-board: a terminal app (Python + Textual) for every Claude Code session on a machine, replacing
Agent View. `README.md` covers use and configuration; `docs/data-sources.md` documents the Claude Code
files it reads and how states are derived from them.

- **Layout:** `board/facts.py` (Claude Code's files → sessions and states), `board/app.py` (the Textual
  app), `board/actions.py` (the `claude` CLI), `board/config.py` (settings), `mod/` (the Claude Code
  mod), `bin/cs` (numbered menu).
- **No hard-coded paths or machine details.** Anything that differs between machines is a setting in
  `board/config.py` (config file plus `SESSION_BOARD_*` variables). Tests must not read the machine's
  own config or `~/.claude`; `tests/conftest.py` sees to the first.
- **Every action goes through the `claude` CLI** (`--bg`, `attach`, `stop`, `rm`). Never edit files under
  `~/.claude`, and never use `claude purge` from the board.
- **Never stop or delete real sessions while testing.** Use throwaway sessions in `mktemp -d` folders and
  remove them with `claude rm` afterwards.
- **Privacy:** never commit transcript-derived data (`*.json` is git-ignored). Mask anything that looks
  like a credential before it is shown or cached (`facts.mask_secrets`).
- **Facts vs judgement:** facts (liveness, git, timing) come from code. Any model-made judgement is
  re-checked against fresh facts before it is shown.
- **`bin/cs`** may be someone's SSH startup command: keep it working, and test with `cs --list` and
  `cs --dry-run N`.
- **Checks:** `.venv/bin/python -m pytest`, and for the mod `claude plugin validate mod/` and
  `claude plugin test mod/`.

**In a copy with `.beads/` and `docs/homelab.md` (the original homelab copy):** read `docs/homelab.md`
and `docs/CONTEXT.md` first. Issues are beads (`bd ready`, `bd show <id>`; see `AGENTS.md` and
`bd prime`), with descriptive ids `sb-<topic>[-<detail>]` under the epic `sb-board-epic`; use `bd` for all
task tracking, not TodoWrite or TaskCreate. Don't commit or push without being asked.
