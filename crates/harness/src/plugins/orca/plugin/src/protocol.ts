// The panel <-> worker contract, plus the shapes both sides share. Types only: no runtime code.

/** How a hookah run ended; `fail` is the worst. */
export type Standing = 'pass' | 'degraded' | 'fail'

/** The latest recorded run of one project (`hookah status --json`, `at_ms` renamed). */
export type StatusEntry = {
  phase: string
  standing: Standing
  problems: number
  detail: string | null
  atMs: number
  lines: string[]
}

export type ProjectRow = { root: string; status: StatusEntry | null }

/** One worktree as the panel shows it. A worktree can hold several projects (a monorepo). */
export type WorktreeRow = {
  workspaceId: string
  name: string
  branch: string | null
  path: string
  /** The worst standing of its projects; null when hookah recorded nothing yet. */
  overall: Standing | null
  problems: number
  /** The newest record time over its projects (ms since the epoch); null when none. */
  atMs: number | null
  /** A `hookah check` started from Orca is running here. */
  running: boolean
  projects: ProjectRow[]
}

declare module 'orca' {
  interface PluginRequests {
    /** Every local worktree with the latest hookah status. `error` explains an empty answer. */
    'hookah.list': { output: { worktrees: WorktreeRow[]; error: string | null } }
    /** Starts `hookah check --changed` in a worktree. `started` is false when one is running. */
    'hookah.check': { input: { workspaceId: string }; output: { started: boolean; error: string | null } }
  }
  interface PluginPanelEvents {
    /** Something changed: ask `hookah.list` again. */
    'hookah.changed': { workspaceId: string | null }
  }
}
