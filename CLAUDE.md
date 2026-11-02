# CLAUDE.md

session-board: a terminal app (Rust + Ratatui) for every Claude Code session on a machine, replacing
Agent View. `README.md` covers use and configuration; `docs/data-sources.md` documents the Claude Code
files it reads and how states are derived from them.

- **Layout:** `src/facts.rs` and `src/claude/` (Claude Code's files → sessions and states),
  `src/actions.rs` (rules and the `claude` CLI), `src/config.rs` (settings), `src/tui/` (the app; views in
  `src/tui/views/`, how they plug in is in its README), `src/shot.rs` (`board --shot`), `mod/` (the Claude
  Code mod, TypeScript).
- **No hard-coded paths or machine details.** Anything that differs between machines is a setting in
  `src/config.rs` (config file plus `SESSION_BOARD_*` variables). Tests must not read the machine's
  own config or `~/.claude`: pass paths and environment explicitly.
- **Every action goes through the `claude` CLI** (`--bg`, `attach`, `stop`, `rm`). Never edit files under
  `~/.claude`, and never use `claude purge` from the board.
- **Never stop or delete real sessions while testing.** Use throwaway sessions in `mktemp -d` folders and
  remove them with `claude rm` afterwards.
- **Privacy:** never commit transcript-derived data (`*.json` is git-ignored). Mask anything that looks
  like a credential before it is shown or cached (`facts.mask_secrets`).
- **Facts vs judgement:** facts (liveness, git, timing) come from code. Any model-made judgement is
  re-checked against fresh facts before it is shown.
- **Terminal handoff:** read input only on the main thread (an input thread would steal keys from
  `claude attach`), and never write to the terminal from a worker thread.
- **Look:** colours only from `src/tui/theme.rs` (amber marks "needs you"); no `Color::Rgb` in views.
- **Checks:** `cargo test`, `cargo clippy --all-targets` (no warnings), and for the mod `claude plugin validate mod/` and
  `claude plugin test mod/`.

**If the copy has `.beads/` or more docs** (a working copy with its own notes): read whatever docs it
has first. Issues are beads (`bd ready`, `bd show <id>`; see `AGENTS.md` and
`bd prime`), with descriptive ids `sb-<topic>[-<detail>]` under the epic `sb-board-epic`; use `bd` for all
task tracking, not TodoWrite or TaskCreate. Don't commit or push without being asked.
