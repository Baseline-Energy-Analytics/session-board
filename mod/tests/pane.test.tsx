import { expect, mock, test } from 'claude-code/testing'
import type { Engine } from 'claude-code/testing'
import type { On } from 'claude-code'

const PANE = {
  plugin: 'session-board',
  component: 'Pane',
  requestId: 'board',
  props: {
    title: 'Sessions',
    isFocused: true,
    bodyColumns: 60,
    placement: 'dock',
    scroll: { offset: 0, bodyRows: 30 },
    view: {},
  },
} as const

const NOW_S = 1_000 // the mock clock's now, in seconds (1_000_000 ms)
const ROWS = [
  { session_id: 'self', job: null, title: 'this one', state: 'elsewhere', project: '/home/r', last: NOW_S - 10 },
  { session_id: 'a', job: 'aaaa1111', title: 'plex update', state: 'needs you', project: '/opt/plex', last: NOW_S - 600,
    model: 'claude-opus-5-5', context: 182_000 },
  { session_id: 'b', job: 'bbbb2222', title: 'media build', state: 'running', project: '/opt/media', last: NOW_S - 30 },
  { session_id: 'c', job: 'cccc3333', title: 'paused thing', state: 'idle', stopped: true, project: '/opt/media', last: NOW_S - 7200 },
  { session_id: 'd', job: null, title: 'old chat', state: 'ended', project: '/opt/plex', last: NOW_S - 90_000 },
]

/** A session with the board answering `rows`, and /board run. */
async function open($: Engine, on: On, rows: unknown[] = ROWS) {
  const clock = mock.clock(on, { now: NOW_S * 1000 })
  mock.env(on, { HOME: '/home/r' })
  on('session.id', () => ({ value: 'self' }))
  on('session.surfaces', () => ({ value: ['terminal'] }))
  on('session.start', (_, e) => ({ cwd: e.cwd }))
  on('command.register', () => ({ value: { command: 'board' } }))
  on('ui.open', () => ({ value: { isPlaced: true } }))
  on('process.run', () => ({
    value: { exitCode: 0, stdout: JSON.stringify(rows), stderr: '', isStdoutTruncated: false, isStderrTruncated: false },
  }))
  await $.session.start({ cwd: '/', surface: 'terminal', isInteractive: true })
  await clock.settle()
  await $.command.run({ command: 'board', args: '', origin: { kind: 'composer' }, presentation: { isFullscreen: true, columns: 160 } })
  await clock.settle()
}

test('lists what needs you first, then live sessions by project; ended only counted', async ($, on) => {
  await open($, on)
  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' })
  const texts = (await ui.findAll({ type: 'Text' })).map(t => t.text)
  const all = texts.join('\n')
  expect(all).toContain('◆ 1 need you')
  expect(all).toContain('1 running')
  expect(all).toContain('1 ended')
  expect(all).not.toContain('old chat')
  // The needs-you row comes before the project headers, with its project.
  const firstNeeds = texts.findIndex(t => t.includes('plex update'))
  const media = texts.findIndex(t => t === 'media')
  expect(firstNeeds).toBeGreaterThan(-1)
  expect(firstNeeds).toBeLessThan(media)
  expect(all).toContain('this one (this)')
  await ui.unmount()
})

test('the details follow the focus ring, and Enter or c copies the attach command', async ($, on) => {
  const copied: string[] = []
  on('ui.copy', (_, e) => {
    copied.push(e.text)
    return { value: { isCopied: true } }
  })
  on('ui.focus', () => ({}))
  await open($, on)
  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' })
  // Before the ring moves, the first session that needs you is shown, at the top.
  expect((await ui.find({ type: 'Text', text: /Opus 5\.5/ }))?.text).toBe('Opus 5.5 · 182k in context')
  await $.ui.focus({ component: 'Pane', requestId: 'board', element: 'project:c', origin: { kind: 'person' } })
  expect((await ui.find({ type: 'Text', text: /^stopped/ }))?.text).toContain('stopped · 2h · media')
  await ui.press({ key: 'copy' }) // c
  await ui.press({ key: 'project:b' }) // Enter on another row copies that one's
  expect(copied).toEqual(['claude attach cccc3333', 'claude attach bbbb2222'])
  await ui.unmount()
})

test('a session with no job says where it is instead of offering a copy', async ($, on) => {
  on('ui.focus', () => ({}))
  await open($, on)
  const ui = await $.ui.mount({ ...PANE, surface: 'terminal' })
  await $.ui.focus({ component: 'Pane', requestId: 'board', element: 'project:self', origin: { kind: 'person' } })
  expect(await ui.find({ type: 'Button', key: 'copy' })).toBeUndefined()
  expect((await ui.find({ type: 'Text', text: /^Open in another/ }))?.text).toBe('Open in another terminal.')
  await ui.unmount()
})
