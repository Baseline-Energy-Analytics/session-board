/** One session that needs you, as the board reports it. */
export type Waiting = { title: string; job: string | null }

/** What the band shows: the other sessions that need you, and when that was read. */
export type Others = { needYou: Waiting[]; at: number }

/** One live session in the /board pane: the fields of `board --json` it draws. */
export type Live = {
  id: string
  job: string | null
  title: string
  state: string
  stopped: boolean
  project: string
  last: number
  model: string | null
  context: number
  isSelf: boolean
}

/** What the /board pane shows: every live session in the board's order, and when it was read. */
export type LiveBoard = { sessions: Live[]; ended: number; at: number; home: string }

declare module 'claude-code' {
  interface PluginState {
    'session-board': {
      others: Shaped<Others | null>
      live: Shaped<LiveBoard | null>
      picked: Shaped<string | null>
    }
  }
}
