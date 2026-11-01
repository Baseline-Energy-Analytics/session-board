import { expect, mock, test } from 'claude-code/testing'
import type { Engine } from 'claude-code/testing'
import type { On, RenderSurface } from 'claude-code'

const BAND = {
  plugin: 'session-board',
  component: 'AbovePrompt',
  props: {
    hasSurvey: false,
    isWorking: false,
    maxRows: 10,
    bodyColumns: 160,
    scroll: { offset: 0, bodyRows: 10 },
    view: {},
  },
} as const

const SELF = 'self-session'
const JOB_DIR = '/home/r/.claude/jobs/selfjob'

type Row = { session_id: string; job: string | null; title: string; state: string }

/** What `board --json` prints, and a record of every time the band ran it. */
function board(on: On, rows: Row[], exitCode = 0) {
  const runs: string[][] = []
  on('process.run', (_, e) => {
    runs.push([...e.argv])
    const stdout = JSON.stringify(rows)
    return { value: { exitCode, stdout, stderr: '', isStdoutTruncated: false, isStderrTruncated: false } }
  })
  return runs
}

/** The engine beneath the band: a session with a terminal, an id, and the environment given. */
async function start($: Engine, on: On, env: Record<string, string>,
                     surfaces: RenderSurface[] = ['terminal']) {
  const clock = mock.clock(on, { now: 1_000_000 })
  mock.env(on, { HOME: '/home/r', ...env })
  on('session.id', () => ({ value: SELF }))
  on('session.surfaces', () => ({ value: surfaces }))
  on('session.start', (_, e) => ({ cwd: e.cwd }))
  // The engine's own band, drawn when the mod passes: so "nothing from the mod" can be seen.
  on('ui.render', ($, e) => {
    const { Text } = $.ui.resolve(e)
    return <Text>engine</Text>
  })
  await $.session.start({ cwd: '/', surface: 'terminal', isInteractive: true })
  await clock.settle()
  return clock
}

const ROWS: Row[] = [
  { session_id: SELF, job: 'selfjob', title: 'this one', state: 'needs you' },
  { session_id: 'after-clear', job: 'selfjob', title: 'this one after /clear', state: 'needs you' },
  { session_id: 'a', job: 'aaaa', title: 'plex update', state: 'needs you' },
  { session_id: 'b', job: 'bbbb', title: 'billsplit review', state: 'needs you' },
  { session_id: 'c', job: 'cccc', title: 'quiet one', state: 'idle' },
  { session_id: 'd', job: null, title: 'old one', state: 'ended' },
]

test('counts the other sessions that need you, never this one', async ($, on) => {
  const runs = board(on, ROWS)
  await start($, on, { CLAUDE_JOB_DIR: JOB_DIR, CLAUDE_CODE_SESSION_KIND: 'bg' })
  expect(runs).toEqual([['/home/r/.local/bin/board', '--json']])

  for (const surface of ['terminal', 'desktop'] as const) {
    const ui = await $.ui.mount({ ...BAND, surface })
    // Excluded: this session by its id, and by its job (a /clear gives the job a new session id).
    expect((await ui.find({ type: 'Text', text: /need you/ }))?.text).toBe(
      '◆ 2 other sessions need you: plex update · billsplit review  ·  Ctrl+Z: board',
    )
    await ui.unmount()
  }
})

test('one other session reads in the singular', async ($, on) => {
  board(on, ROWS.slice(0, 3))
  await start($, on, { CLAUDE_JOB_DIR: JOB_DIR, CLAUDE_CODE_SESSION_KIND: 'bg' })
  const ui = await $.ui.mount({ ...BAND, surface: 'terminal' })
  expect((await ui.find({ type: 'Text', text: /needs you/ }))?.text).toBe(
    '◆ 1 other session needs you: plex update  ·  Ctrl+Z: board',
  )
  await ui.unmount()
})

test('shows nothing when no other session needs you', async ($, on) => {
  board(on, [ROWS[0]!, ROWS[4]!, ROWS[5]!])
  await start($, on, { CLAUDE_JOB_DIR: JOB_DIR, CLAUDE_CODE_SESSION_KIND: 'bg' })
  const ui = await $.ui.mount({ ...BAND, surface: 'terminal' })
  expect(await ui.find({ type: 'Text', text: /need/ })).toBeUndefined()
  expect((await ui.find({ type: 'Text', text: /engine/ }))?.text).toBe('engine')
  await ui.unmount()
})

test('offers Ctrl+Z only in a background session', async ($, on) => {
  // An interactive session has no job, so only its own session id is left out.
  board(on, ROWS)
  await start($, on, {})
  const ui = await $.ui.mount({ ...BAND, surface: 'terminal' })
  expect((await ui.find({ type: 'Text', text: /need you/ }))?.text).toBe(
    '◆ 3 other sessions need you: this one after /clear · plex update · billsplit review',
  )
  await ui.unmount()
})

test('runs the board named in SESSION_BOARD_BIN', async ($, on) => {
  const runs = board(on, ROWS)
  await start($, on, { SESSION_BOARD_BIN: '/opt/tools/board' })
  expect(runs).toEqual([['/opt/tools/board', '--json']])
})

test('shows nothing when the board fails', async ($, on) => {
  board(on, ROWS, 1)
  await start($, on, { CLAUDE_CODE_SESSION_KIND: 'bg' })
  const ui = await $.ui.mount({ ...BAND, surface: 'terminal' })
  expect(await ui.find({ type: 'Text', text: /need/ })).toBeUndefined()
  await ui.unmount()
})

test('re-reads the board every 30 s, and never in a headless run', async ($, on) => {
  const runs = board(on, ROWS)
  const clock = await start($, on, { CLAUDE_CODE_SESSION_KIND: 'bg' }, [])
  await clock.advance(65_000)
  expect(runs).toEqual([])
})

test('re-reads the board every 30 s while something draws', async ($, on) => {
  const runs = board(on, ROWS)
  const clock = await start($, on, { CLAUDE_CODE_SESSION_KIND: 'bg' })
  await clock.advance(65_000)
  expect(runs.length).toBe(3)
})
