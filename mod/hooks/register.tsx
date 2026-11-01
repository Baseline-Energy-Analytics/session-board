// Companion to the session board, a terminal app that replaces Agent View.
//
// It runs `board --json`: the command in SESSION_BOARD_BIN if set (put it in the `env` block of
// ~/.claude/settings.json), else ~/.local/bin/board.
//
// A band above the prompt: "◆ N other sessions need you: <titles>", and in a background session the
// way back to the board, "Ctrl+Z: board". Nothing when no other session needs Ryan.
//
// What it can't do (checked on Claude Code 2.1.316; details in the sb-board-mod bead):
// - Redirect ←. It opens Agent View from the prompt's own key handling, not through a keybinding
//   action a mod Button could take over (and a Button's `action` only answers chords or modified keys).
// - End `claude attach`. No call on `$` makes a client leave. Claude Code's /exit in a background
//   session, typed or run by a mod, detaches into Agent View, so a /board command would only have
//   been /exit under another name. Ctrl+Z is the one key that goes straight back to the board.
import { atom, read, update } from 'claude-code'
import type { EngineInterface, Register } from 'claude-code'

import type { Others, Waiting } from '../types'

/** How often the band re-reads the board; `board --json` takes about 0.3 s. */
const POLL_MS = 30_000

const others = atom({ plugin: 'session-board', key: 'others' } as const, null, {
  shape: 'others-1',
})

/** One row of `board --json`, the fields the band uses. */
type Row = { session_id: string; job: string | null; title: string; state: string }

/**
 * The other sessions that need Ryan, or null when the board can't be read. The one place the band
 * gets its facts: `board --json` today; sb-tui-live will add a hooks-fed state folder to read instead.
 */
async function readOthers($: EngineInterface): Promise<Others | null> {
  const home = await $.env.get('HOME')
  const jobDir = await $.env.get('CLAUDE_JOB_DIR')
  const self = await $.session.id()
  const myJob = jobDir?.split('/').pop()
  const board = (await $.env.get('SESSION_BOARD_BIN')) || `${home ?? ''}/.local/bin/board`
  const run = await $.process.run([board, '--json'], { timeoutMs: 20_000 })
  if (run.exitCode !== 0) {
    return null
  }
  let rows: Row[]
  try {
    rows = JSON.parse(run.stdout) as Row[]
  } catch {
    return null
  }
  const needYou: Waiting[] = rows
    .filter(s => s.state === 'needs you' && s.session_id !== self && !(myJob && s.job === myJob))
    .map(s => ({ title: s.title, job: s.job }))

  return { needYou, at: await $.clock.now() }
}

export const register: Register = on => {
  on('session.start', async ($, e, next) => {
    let isReading = false
    const refresh = async () => {
      // Nothing draws in a headless run (`claude -p`, the SDK): don't read the board for it.
      if (isReading || (await $.session.surfaces()).length === 0) {
        return
      }
      isReading = true
      try {
        const now = await readOthers($)
        await update($, others, () => now)
      } catch {
        await update($, others, () => null)
      } finally {
        isReading = false
      }
    }
    void refresh()
    $.clock.every(POLL_MS, () => {
      void refresh()
    })

    return next(e)
  })

  on('ui.render', { component: 'AbovePrompt' }, async ($, e, next) => {
    const now = await read($, others)
    if (e.props.hasSurvey || now === null || now.needYou.length === 0) {
      return next(e)
    }

    const { Box, Text } = $.ui.resolve(e)
    const n = now.needYou.length
    const head = n === 1 ? '1 other session needs you' : `${n} other sessions need you`
    const titles = now.needYou.map(w => w.title).join(' · ')
    // Ctrl+Z ends `claude attach` only in a background session; elsewhere it suspends Claude Code.
    const isBackground = (await $.env.get('CLAUDE_CODE_SESSION_KIND')) === 'bg'

    return (
      <Box>
        <Text wrap="truncate-end">
          <Text color="warning" bold>
            ◆ {head}
          </Text>
          <Text dimColor>
            : {titles}
            {isBackground ? '  ·  Ctrl+Z: board' : ''}
          </Text>
        </Text>
      </Box>
    )
  })
}
