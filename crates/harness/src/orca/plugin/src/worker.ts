// The plugin's worker. It never edits anything on its own: per-edit feedback comes from the hooks
// of the agent Orca hosts (`hookah setup <agent>`). The worker adds what only Orca can: it reads
// hookah's latest verdict per worktree, runs a check on demand, and (when the user allows it)
// sets hookah up for an agent right before Orca starts it.
import type { OrcaWorkerApi } from 'orca/worker'
import {
  candidates,
  describe,
  exec,
  harnessFor,
  hookahClient,
  label,
  needsSetup,
  summarize,
  type Exec,
  type Hookah
} from './hookah'
import type { ProjectRow, WorktreeRow } from './protocol'

/** A worktree on this machine, as `workspace.list` reports it. */
type Worktree = { id: string; name: string; branch: string | null; path: string }

type Settings = { hookahPath: string; setup: 'suggest' | 'auto' | 'off'; checkTimeoutSeconds: number }

/** Tests replace the process runner. */
export const runner: { exec: Exec } = { exec }

const MAX_WORKTREES = 40
const PARALLEL = 4

export default function activate(orca: OrcaWorkerApi): void {
  const running = new Set<string>()
  const told = new Set<string>()

  async function settings(): Promise<Settings> {
    const all = (await orca.settings.get()) as Record<string, unknown>
    const setup = all.setup === 'auto' || all.setup === 'off' ? all.setup : 'suggest'
    const timeout = typeof all.checkTimeoutSeconds === 'number' ? all.checkTimeoutSeconds : 300
    return {
      hookahPath: typeof all.hookahPath === 'string' && all.hookahPath !== '' ? all.hookahPath : 'hookah',
      setup,
      checkTimeoutSeconds: Math.min(1800, Math.max(10, timeout))
    }
  }

  /** The hookah client for the first candidate program that starts. */
  async function hookah(): Promise<Hookah> {
    const configured = (await settings()).hookahPath
    const env = (globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env ?? {}
    for (const bin of candidates(configured, env)) {
      const probe = await runner.exec(bin, ['--version'], { cwd: '.', timeoutMs: 5000 })
      if (probe.code === 0) return hookahClient(runner.exec, bin)
    }
    throw new Error(`cannot run \`${configured}\`: install hookah or set its path in the plugin settings`)
  }

  async function worktrees(): Promise<Worktree[]> {
    const { workspaces } = await orca.host.call('workspace.list')
    return workspaces
      .filter((w) => w.kind === 'worktree' && (w.hostId === null || w.hostId === 'local'))
      .map((w) => ({ id: w.id, name: w.name, branch: w.branch, path: w.path }))
  }

  /** The worktree of an event or of the focused workspace; the ids of the two lists may differ,
   *  so fall back to a unique branch or name. */
  function find(list: Worktree[], id: string | null | undefined, branch?: string, name?: string): Worktree | null {
    const byId = id ? list.find((w) => w.id === id) : undefined
    if (byId) return byId
    const byBranch = branch ? list.filter((w) => w.branch === branch) : []
    if (byBranch.length === 1) return byBranch[0]
    const byName = name ? list.filter((w) => w.name === name) : []
    return byName.length === 1 ? byName[0] : null
  }

  async function focused(): Promise<Worktree | null> {
    const ctx = await orca.host.call('workspace.readContext')
    if (ctx === null) return null
    return find(await worktrees(), ctx.workspaceId, ctx.branch, ctx.displayName)
  }

  async function projects(path: string): Promise<ProjectRow[]> {
    return (await hookah()).status(path)
  }

  function row(w: Worktree, rows: ProjectRow[]): WorktreeRow {
    return { workspaceId: w.id, name: w.name, branch: w.branch, path: w.path, running: running.has(w.id), projects: rows, ...summarize(rows) }
  }

  function changed(workspaceId: string | null): void {
    orca.panels.post('hookah.changed', { workspaceId })
    void refreshStatusBar()
  }

  async function refreshStatusBar(): Promise<void> {
    try {
      const w = await focused()
      if (w === null) {
        await orca.host.call('statusBar.update', { id: 'status', text: 'hookah', tone: 'neutral', icon: 'info', tooltip: 'hookah: no worktree focused' })
        return
      }
      const rows = await projects(w.path)
      const l = label(summarize(rows))
      await orca.host.call('statusBar.update', { id: 'status', text: l.text, tone: l.tone, icon: l.icon, tooltip: describe(w.name, rows) })
    } catch (e) {
      const message = e instanceof Error ? e.message : String(e)
      orca.log(`status bar: ${message}`)
      await orca.host.call('statusBar.update', { id: 'status', text: 'hookah: unavailable', tone: 'warning', icon: 'warning', tooltip: message }).catch(() => undefined)
    }
  }

  /** Runs `hookah check --changed` in the background; false when one is running already. */
  function startCheck(w: Worktree): boolean {
    if (running.has(w.id)) return false
    running.add(w.id)
    changed(w.id)
    void (async () => {
      let message: string
      try {
        const client = await hookah()
        const timeout = (await settings()).checkTimeoutSeconds * 1000
        const result = await client.check(w.path, timeout)
        message = result.output.trim() || (result.passed ? 'ok' : 'failed')
      } catch (e) {
        message = e instanceof Error ? e.message : String(e)
      } finally {
        running.delete(w.id)
      }
      changed(w.id)
      await orca.host.call('notifications.show', { title: `hookah: ${w.name}`, body: message.slice(0, 400) }).catch(() => undefined)
    })()
    return true
  }

  // ---- panel -------------------------------------------------------------------------------

  orca.panels.handle('hookah.list', async () => {
    try {
      const list = (await worktrees()).slice(0, MAX_WORKTREES)
      const client = await hookah()
      const rows: WorktreeRow[] = new Array(list.length)
      let next = 0
      const worker = async (): Promise<void> => {
        for (let i = next++; i < list.length; i = next++) {
          rows[i] = row(list[i], await client.status(list[i].path).catch(() => []))
        }
      }
      await Promise.all(Array.from({ length: Math.min(PARALLEL, list.length) }, worker))
      return { worktrees: rows, error: null }
    } catch (e) {
      return { worktrees: [], error: e instanceof Error ? e.message : String(e) }
    }
  })

  orca.panels.handle('hookah.check', async ({ workspaceId }) => {
    const w = (await worktrees()).find((x) => x.id === workspaceId)
    if (w === undefined) return { started: false, error: 'that worktree is gone' }
    return { started: startCheck(w), error: null }
  })

  // ---- palette commands --------------------------------------------------------------------

  orca.commands.register('hookah.status', async () => {
    const w = await focused()
    if (w === null) return 'hookah: no worktree is focused'
    return describe(w.name, await projects(w.path))
  })

  orca.commands.register('hookah.check', async () => {
    const w = await focused()
    if (w === null) return 'hookah: no worktree is focused'
    return startCheck(w)
      ? `hookah: checking ${w.name}; a notification shows the result`
      : `hookah: a check is already running in ${w.name}`
  })

  orca.commands.register('hookah.setup', async () => {
    const w = await focused()
    if (w === null) return 'hookah: no worktree is focused'
    const client = await hookah()
    const out: string[] = []
    for (const d of await client.detect(w.path)) {
      if (!needsSetup(d)) continue
      if (d.project === 'none') {
        out.push(`${d.harness}: has no project config; run \`hookah setup ${d.harness}\` yourself to set up your user config`)
        continue
      }
      await client.setup(w.path, d.harness)
      out.push(`${d.harness}: wrote its project config in ${w.name}`)
    }
    if (out.length === 0) return 'hookah: nothing to set up, every installed agent has hookah hooks already'
    out.push('Restart the agent so it reads the new hooks.')
    changed(w.id)
    return out.join('\n')
  })

  // ---- events ------------------------------------------------------------------------------

  /** The stop hook may still be writing its verdict when the agent reports done: look again. */
  function settle(workspaceId: string | null): void {
    for (const ms of [1500, 8000]) {
      const timer = setTimeout(() => changed(workspaceId), ms) as { unref?: () => void } & ReturnType<typeof setTimeout>
      timer.unref?.()
    }
  }

  orca.events.on('agent.status.changed', (e) => {
    if (e.state === 'done' || e.state === 'waiting') {
      changed(e.workspaceListId ?? e.workspaceId)
      settle(e.workspaceListId ?? e.workspaceId)
    }
  })
  orca.events.on('workspace.changed', () => void refreshStatusBar())
  orca.events.on('workspace.created', (e) => changed(e.workspaceListId ?? e.workspaceId))
  orca.events.on('workspace.removed', (e) => changed(e.workspaceListId ?? e.workspaceId))

  // ---- hook: set hookah up before an agent starts ------------------------------------------

  /** Never denies and never changes the launch, and never throws: the hook only prepares. */
  orca.hooks.on('agent.launch.before', async (event) => {
    try {
      const harness = harnessFor(event.agent)
      if (harness === null || event.workspaceKind !== 'worktree' || event.workspaceId === null) return
      const mode = (await settings()).setup
      if (mode === 'off') return
      const w = find(await worktrees(), event.workspaceId)
      if (w === null) return
      const client = await hookah()
      const d = (await client.detect(w.path)).find((x) => x.harness === harness)
      if (d === undefined || !needsSetup(d)) return
      const key = `${w.path}|${harness}`
      if (mode === 'auto' && d.project !== 'none') {
        await client.setup(w.path, harness)
        told.add(key)
        void orca.host.call('notifications.show', {
          title: `hookah: set up for ${harness}`,
          body: `Wrote the ${harness} project config in ${w.name}. Review it before you commit.`
        })
      } else if (!told.has(key)) {
        told.add(key)
        void orca.host.call('notifications.show', {
          title: `hookah is not set up for ${harness}`,
          body: `Run "hookah: set up hooks for this worktree" in ${w.name}, or \`hookah setup ${harness}\` in a terminal.`
        })
      }
    } catch (e) {
      orca.log(`agent.launch.before: ${e instanceof Error ? e.message : String(e)}`)
    }
  })

  void refreshStatusBar()
}
