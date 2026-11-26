// Runs src/worker.ts's activate(orca) over a mock Orca host and a fake hookah.
import { assert, mockHost, mountWorker, test } from 'orca/testing'
import type { Exec } from '../src/hookah'
import * as main from '../src/worker'

const WORKTREES = {
  workspaces: [
    { id: 'w1', kind: 'worktree', name: 'feat', projectId: 'p', path: '/repo/feat', branch: 'feat', hostId: 'local' },
    { id: 'w2', kind: 'worktree', name: 'fix', projectId: 'p', path: '/repo/fix', branch: 'fix', hostId: 'local' },
    { id: 'w3', kind: 'worktree', name: 'far', projectId: 'p', path: '/srv/far', branch: 'far', hostId: 'ssh:box' },
    { id: 'f1', kind: 'folder', name: 'notes', projectId: 'p', path: '/repo/notes', branch: null, hostId: 'local' }
  ],
  truncated: false
}

const FAILING = {
  v: 1,
  root: '/repo/feat',
  overall: 'fail',
  projects: [
    { root: '/repo/feat', status: { phase: 'stop', standing: 'fail', problems: 2, detail: null, at_ms: 5, lines: ['a.rs:1: x'] } }
  ]
}
const NEVER = { v: 1, root: '/repo/fix', overall: null, projects: [{ root: '/repo/fix', status: null }] }

const DETECT_MISSING =
  'claude: installed (binary /bin/claude); project: not configured; user: not configured\n' +
  'antigravity: installed (binary /bin/agy); project: not configured; user: not configured\n' +
  'zcode: installed (binary /bin/zcode); project: not configured; user: not configured\n' +
  'kimi: installed (binary /bin/kimi); project: no project config; user: not configured\n' +
  'codex: installed (binary /bin/codex); project: not configured; user: configured\n'

/** Records every hookah call and answers like hookah would. */
function fakeHookah(): { calls: string[]; exec: Exec } {
  const calls: string[] = []
  const exec: Exec = async (_file, args, options) => {
    calls.push([...args, options.cwd].join(' '))
    const ok = (stdout: string) => ({ code: 0, stdout, stderr: '', failed: null })
    if (args[0] === '--version') return ok('hookah 0.2.0\n')
    if (args[0] === 'status') return ok(JSON.stringify(options.cwd === '/repo/feat' ? FAILING : NEVER))
    if (args[0] === 'setup' && args[1] === '--detect') return ok(DETECT_MISSING)
    if (args[0] === 'setup') return ok('created settings\n')
    if (args[0] === 'check') return { code: 1, stdout: 'FAIL .\n  a.rs:1: x\n', stderr: '', failed: null }
    return { code: 2, stdout: '', stderr: 'unexpected\n', failed: null }
  }
  return { calls, exec }
}

async function setupWorker(settings: Record<string, string | number> = {}, focused: string | null = 'w1') {
  const fake = fakeHookah()
  main.runner.exec = fake.exec
  const host = mockHost({
    settings,
    results: {
      'workspace.list': WORKTREES,
      'workspace.readContext': focused === null ? null : { workspaceId: focused, branch: 'feat', displayName: 'feat', terminals: [] },
      'statusBar.update': { ok: true },
      'notifications.show': { delivered: true }
    }
  })
  const worker = await mountWorker(main, { host })
  await tick()
  return { worker, host, fake }
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 20))

function launch(agent: string, overrides: Record<string, unknown> = {}) {
  return { agent, workspaceId: 'w1', workspaceKind: 'worktree' as const, command: agent, env: {}, ...overrides }
}

function notices(host: { callsOf(method: string): unknown[] }): string[] {
  return host.callsOf('notifications.show').map((p) => (p as { title: string }).title)
}

test('the status bar shows the focused worktree', async () => {
  const { worker, host } = await setupWorker()
  const updates = host.callsOf('statusBar.update') as { text?: string; tone?: string; tooltip?: string }[]
  const last = updates[updates.length - 1]
  assert.equal(last.text, 'hookah: 2 problems')
  assert.equal(last.tone, 'danger')
  assert.match(String(last.tooltip), /a\.rs:1: x/)
  assert.deepEqual(worker.refusals, [])
  await worker.deactivate()
})

test('without a focused worktree the status bar stays neutral', async () => {
  const { worker, host } = await setupWorker({}, null)
  const updates = host.callsOf('statusBar.update') as { text?: string }[]
  assert.equal(updates[updates.length - 1].text, 'hookah')
  await worker.deactivate()
})

test('the panel lists local worktrees only, with their status', async () => {
  const { worker } = await setupWorker()
  const answer = await worker.request('hookah.list')
  assert.equal(answer.error, null)
  assert.deepEqual(
    answer.worktrees.map((w) => [w.workspaceId, w.overall, w.problems, w.running]),
    [
      ['w1', 'fail', 2, false],
      ['w2', null, 0, false]
    ]
  )
  await worker.deactivate()
})

test('a missing hookah is reported, not thrown', async () => {
  const { worker } = await setupWorker()
  main.runner.exec = async () => ({ code: null, stdout: '', stderr: '', failed: 'spawn hookah ENOENT' })
  const answer = await worker.request('hookah.list')
  assert.deepEqual(answer.worktrees, [])
  assert.match(String(answer.error), /cannot run `hookah`/)
  await worker.deactivate()
})

test('the status command prints the report of the focused worktree', async () => {
  const { worker } = await setupWorker()
  const text = String(await worker.command('hookah.status'))
  assert.match(text, /^feat: 2 problems after the last stop check/)
  await worker.deactivate()
})

test('the check command runs in the background and notifies', async () => {
  const { worker, host, fake } = await setupWorker()
  let finishCheck!: () => void
  const checkInProgress = new Promise<void>((resolve) => { finishCheck = resolve })
  main.runner.exec = async (file, args, options) => {
    if (args[0] === 'check') await checkInProgress
    return fake.exec(file, args, options)
  }
  try {
    const first = String(await worker.command('hookah.check'))
    assert.match(first, /checking feat/)
    assert.match(String(await worker.command('hookah.check')), /already running/)
  } finally {
    finishCheck()
  }
  await tick()
  assert.ok(fake.calls.includes('check --changed --root /repo/feat /repo/feat'))
  assert.deepEqual(notices(host), ['hookah: feat'])
  assert.equal(worker.posts.some((p) => p.event === 'hookah.changed'), true)
  await worker.deactivate()
})

test('the setup command writes project config only for agents that have one', async () => {
  const { worker, fake } = await setupWorker()
  const text = String(await worker.command('hookah.setup'))
  assert.match(text, /claude: wrote its project config in feat/)
  assert.match(text, /antigravity: wrote its project config in feat/)
  assert.match(text, /zcode: wrote its project config in feat/)
  assert.match(text, /kimi: has no project config/)
  assert.equal(text.includes('codex'), false)
  assert.deepEqual(
    fake.calls.filter((c) => c.startsWith('setup claude') || c.startsWith('setup kimi')),
    ['setup claude --scope project /repo/feat']
  )
  await worker.deactivate()
})

test('suggest mode only notifies, once per worktree and agent', async () => {
  const { worker, host, fake } = await setupWorker()
  for (let i = 0; i < 2; i++) {
    const decision = await worker.hooks.fire('agent.launch.before', launch('claude'))
    assert.equal(decision.kind, 'continue')
  }
  await tick()
  assert.deepEqual(notices(host), ['hookah is not set up for claude'])
  assert.equal(fake.calls.some((c) => c.startsWith('setup claude')), false)
  await worker.deactivate()
})

test('auto mode sets up the agent before it starts, and tells the user', async () => {
  const { worker, host, fake } = await setupWorker({ setup: 'auto' })
  const decision = await worker.hooks.fire('agent.launch.before', launch('claude'))
  assert.deepEqual(decision, { kind: 'continue', event: launch('claude') })
  assert.ok(fake.calls.includes('setup claude --scope project /repo/feat'))
  await tick()
  assert.deepEqual(notices(host), ['hookah: set up for claude'])
  await worker.deactivate()
})

test('auto mode resolves the launch hook worktree ID to the workspace list entry', async () => {
  const { worker, fake } = await setupWorker({ setup: 'auto' })
  const otherProject = launch('claude', { workspaceId: 'other-project::/repo/feat' })
  assert.deepEqual(await worker.hooks.fire('agent.launch.before', otherProject), { kind: 'continue', event: otherProject })
  assert.equal(fake.calls.some((c) => c.startsWith('setup claude')), false)

  const event = launch('claude', { workspaceId: 'p::/repo/feat' })
  assert.deepEqual(await worker.hooks.fire('agent.launch.before', event), { kind: 'continue', event })
  assert.ok(fake.calls.includes('setup claude --scope project /repo/feat'))
  await worker.deactivate()
})

test('auto mode never writes the user config for an agent without a project config', async () => {
  const { worker, host, fake } = await setupWorker({ setup: 'auto' })
  await worker.hooks.fire('agent.launch.before', launch('kimi'))
  await tick()
  assert.equal(fake.calls.some((c) => c.startsWith('setup kimi')), false)
  assert.deepEqual(notices(host), ['hookah is not set up for kimi'])
  await worker.deactivate()
})

test('nothing happens when it is off, configured, unknown, or not a worktree', async () => {
  const off = await setupWorker({ setup: 'off' })
  await off.worker.hooks.fire('agent.launch.before', launch('claude'))
  assert.equal(off.fake.calls.some((c) => c.startsWith('setup')), false)
  await off.worker.deactivate()

  const { worker, host, fake } = await setupWorker({ setup: 'auto' })
  for (const event of [launch('codex'), launch('gemini'), launch('claude', { workspaceKind: 'folder' }), launch('claude', { workspaceId: null })]) {
    assert.equal((await worker.hooks.fire('agent.launch.before', event)).kind, 'continue')
  }
  await tick()
  assert.equal(fake.calls.some((c) => c.startsWith('setup codex') || c.startsWith('setup claude')), false)
  assert.deepEqual(notices(host), [])
  await worker.deactivate()
})

test('the launch hook passes when hookah fails', async () => {
  const { worker } = await setupWorker({ setup: 'auto' })
  main.runner.exec = async () => ({ code: null, stdout: '', stderr: '', failed: 'spawn hookah ENOENT' })
  const decision = await worker.hooks.fire('agent.launch.before', launch('claude'))
  assert.equal(decision.kind, 'continue')
  assert.ok(worker.logs.some((l) => l.includes('agent.launch.before')))
  await worker.deactivate()
})

test('an agent that finishes makes the panel look again', async () => {
  const { worker } = await setupWorker()
  const before = worker.posts.length
  await worker.emit('agent.status.changed', {
    workspaceId: 'w1',
    workspaceListId: 'w1',
    paneKey: 'pane',
    state: 'done',
    receivedAt: 1
  })
  assert.ok(worker.posts.length > before)
  const quiet = worker.posts.length
  await worker.emit('agent.status.changed', {
    workspaceId: 'w1',
    workspaceListId: 'w1',
    paneKey: 'pane',
    state: 'working',
    receivedAt: 2
  })
  assert.equal(worker.posts.length, quiet)
  await worker.deactivate()
})
