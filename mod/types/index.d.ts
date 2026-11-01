/** One session that needs you, as the board reports it. */
export type Waiting = { title: string; job: string | null }

/** What the band shows: the other sessions that need you, and when that was read. */
export type Others = { needYou: Waiting[]; at: number }

declare module 'claude-code' {
  interface PluginState {
    'session-board': { others: Shaped<Others | null> }
  }
}
