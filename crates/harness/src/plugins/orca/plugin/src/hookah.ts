// Everything that talks to the `hookah` binary, and the pure functions that read its output.
// The worker injects the process runner, so tests run without hookah installed.
import type { ProjectRow, Standing, StatusEntry } from './protocol'

export type ExecResult = { code: number | null; stdout: string; stderr: string; failed: string | null }
export type Exec = (
  file: string,
  args: string[],
  options: { cwd: string; timeoutMs: number }
) => Promise<ExecResult>

/** Runs a program without a shell. The module name sits in a variable so type-checking does not
 *  need the Node typings. Never rejects: a start failure or a timeout is in `failed`. */
export const exec: Exec = async (file, args, { cwd, timeoutMs }) => {
  const specifier = 'node:child_process'
  const { execFile } = (await import(specifier)) as {
    execFile: (
      file: string,
      args: string[],
      options: Record<string, unknown>,
      done: (error: { message: string; code?: unknown } | null, stdout: string, stderr: string) => void
    ) => unknown
  }
  return new Promise((resolve) => {
    execFile(
      file,
      args,
      { cwd, timeout: timeoutMs, maxBuffer: 4 * 1024 * 1024, windowsHide: true },
      (error, stdout, stderr) => {
        const code = error === null ? 0 : typeof error.code === 'number' ? error.code : null
        // A non-zero exit is an answer, not a failure: only a program that never ran, or was
        // killed, has no code.
        resolve({ code, stdout, stderr, failed: code === null && error !== null ? error.message : null })
      }
    )
  })
}

// ---- reading hookah's output ---------------------------------------------------------------

/** What `hookah setup --detect` says about one agent harness. */
export type Detection = {
  harness: string
  installed: boolean
  /** `none`: the harness has no project config (Kimi Code). */
  project: 'configured' | 'missing' | 'none'
  user: boolean
}

const DETECT_LINE =
  /^([a-z]+): (.*); project: (configured|not configured|no project config); user: (configured|not configured)$/

/** Parses the lines of `hookah setup --detect`; anything else (the `next:` hint) is skipped. */
export function parseDetect(text: string): Detection[] {
  const found: Detection[] = []
  for (const line of text.split(/\r?\n/)) {
    const m = DETECT_LINE.exec(line.trim())
    if (m === null) continue
    const [, harness, installed, project, user] = m
    found.push({
      harness,
      installed: installed.startsWith('installed'),
      project: project === 'configured' ? 'configured' : project === 'not configured' ? 'missing' : 'none',
      user: user === 'configured'
    })
  }
  return found
}

/** The scope that still needs hooks for an installed agent launched by Orca. */
export function setupScope(d: Detection): 'project' | 'user' | null {
  if (!d.installed || d.user) return null
  // Orca starts ZCode's standalone CLI, which loads hooks from the user config.
  if (d.harness === 'zcode' || d.project === 'none') return 'user'
  return d.project === 'missing' ? 'project' : null
}

const STANDINGS: readonly string[] = ['pass', 'degraded', 'fail']

function asStatus(value: unknown): StatusEntry | null {
  if (typeof value !== 'object' || value === null) return null
  const v = value as Record<string, unknown>
  if (typeof v.standing !== 'string' || !STANDINGS.includes(v.standing)) return null
  return {
    phase: typeof v.phase === 'string' ? v.phase : 'edit',
    standing: v.standing as Standing,
    problems: typeof v.problems === 'number' ? v.problems : 0,
    detail: typeof v.detail === 'string' ? v.detail : null,
    atMs: typeof v.at_ms === 'number' ? v.at_ms : 0,
    lines: Array.isArray(v.lines) ? v.lines.filter((l): l is string => typeof l === 'string') : []
  }
}

/** Parses `hookah status --json`. Throws when the text is not that document. */
export function parseStatus(text: string): ProjectRow[] {
  const doc = JSON.parse(text) as { v?: unknown; projects?: unknown }
  if (doc.v !== 1 || !Array.isArray(doc.projects)) throw new Error('unexpected `hookah status` output')
  return doc.projects.map((p: { root?: unknown; status?: unknown }) => ({
    root: String(p.root),
    status: asStatus(p.status)
  }))
}

/** The worst standing, the problem count, and the newest time over the recorded projects. */
export function summarize(projects: ProjectRow[]): {
  overall: Standing | null
  problems: number
  atMs: number | null
} {
  const order: Standing[] = ['pass', 'degraded', 'fail']
  let overall: Standing | null = null
  let problems = 0
  let atMs: number | null = null
  for (const { status } of projects) {
    if (status === null) continue
    if (overall === null || order.indexOf(status.standing) > order.indexOf(overall)) overall = status.standing
    problems += status.problems
    if (atMs === null || status.atMs > atMs) atMs = status.atMs
  }
  return { overall, problems, atMs }
}

/** The Orca agent names hookah can set up, mapped to `hookah setup` names. */
const HARNESSES: Record<string, string> = {
  antigravity: 'antigravity',
  claude: 'claude',
  codex: 'codex',
  qoder: 'qoder',
  'qoder-cn': 'qoder',
  kimi: 'kimi',
  opencode: 'opencode',
  zcode: 'zcode'
}

export function harnessFor(agent: string): string | null {
  return HARNESSES[agent] ?? null
}

// ---- running hookah ------------------------------------------------------------------------

const QUICK_MS = 10_000

/** The first non-empty line of a failed run, for a message. */
function reason(r: ExecResult): string {
  if (r.failed !== null) return r.failed
  const line = (r.stderr || r.stdout).split(/\r?\n/).find((l) => l.trim() !== '')
  const text = line?.trim() ?? `exit code ${r.code}`
  return /unrecognized subcommand|unexpected argument/.test(text) ? 'this hookah is too old: update it' : text
}

export type Hookah = ReturnType<typeof hookahClient>

/** A client for the executable at `bin`. */
export function hookahClient(run: Exec, bin: string) {
  return {
    /** The latest recorded status of the projects under `dir`. */
    async status(dir: string): Promise<ProjectRow[]> {
      const r = await run(bin, ['status', '--json', '--root', dir], { cwd: dir, timeoutMs: QUICK_MS })
      if (r.code !== 0) throw new Error(reason(r))
      return parseStatus(r.stdout)
    },
    /** Which harnesses are installed and configured, seen from `dir`. */
    async detect(dir: string): Promise<Detection[]> {
      const r = await run(bin, ['setup', '--detect'], { cwd: dir, timeoutMs: QUICK_MS })
      if (r.code !== 0) throw new Error(reason(r))
      return parseDetect(r.stdout)
    },
    /** Writes the project config of `harness` in `dir`; returns hookah's own report. */
    async setup(dir: string, harness: string): Promise<string> {
      const r = await run(bin, ['setup', harness, '--scope', 'project'], { cwd: dir, timeoutMs: QUICK_MS })
      if (r.code !== 0) throw new Error(reason(r))
      return r.stdout
    },
    /** Checks everything changed in `dir`: static checks, related tests. Exit 1 is a failed check. */
    async check(dir: string, timeoutMs: number): Promise<{ passed: boolean; output: string }> {
      const r = await run(bin, ['check', '--changed', '--root', dir], { cwd: dir, timeoutMs })
      if (r.failed !== null && r.code === null) throw new Error(r.failed)
      if (r.code !== 0 && r.code !== 1) throw new Error(reason(r))
      return { passed: r.code === 0, output: r.stdout }
    }
  }
}

/** The programs to try for the configured command: it, then where the installers put hookah, for
 *  a desktop app whose PATH is short. */
export function candidates(configured: string, env: Record<string, string | undefined>): string[] {
  const list = [configured]
  if (configured === 'hookah') {
    if (env.HOME) list.push(`${env.HOME}/.local/bin/hookah`)
    if (env.LOCALAPPDATA) list.push(`${env.LOCALAPPDATA}\\hookah\\bin\\hookah.exe`)
  }
  return list
}

// ---- what to show --------------------------------------------------------------------------

export type Label = {
  text: string
  tone: 'neutral' | 'success' | 'warning' | 'danger'
  icon: 'check' | 'warning' | 'error' | 'info'
}

/** The status bar label for a summary. */
export function label(s: { overall: Standing | null; problems: number }): Label {
  switch (s.overall) {
    case 'fail':
      return {
        text: `hookah: ${s.problems} ${s.problems === 1 ? 'problem' : 'problems'}`,
        tone: 'danger',
        icon: 'error'
      }
    case 'degraded':
      return { text: 'hookah: skipped', tone: 'warning', icon: 'warning' }
    case 'pass':
      return { text: 'hookah: ok', tone: 'success', icon: 'check' }
    default:
      return { text: 'hookah', tone: 'neutral', icon: 'info' }
  }
}

/** A plain-text report of a worktree, for the status command and the tooltip. */
export function describe(name: string, projects: ProjectRow[]): string {
  const recorded = projects.filter((p) => p.status !== null)
  if (recorded.length === 0) return `${name}: hookah has not checked anything here yet`
  const out: string[] = []
  for (const { root, status } of recorded) {
    if (status === null) continue
    const head =
      status.standing === 'fail'
        ? `${status.problems} ${status.problems === 1 ? 'problem' : 'problems'}`
        : status.standing === 'degraded'
          ? `skipped${status.detail === null ? '' : ` (${status.detail})`}`
          : 'ok'
    out.push(`${recorded.length > 1 ? `${root}: ` : `${name}: `}${head} after the last ${status.phase} check`)
    out.push(...status.lines.map((l) => `  ${l}`))
  }
  return out.join('\n')
}
