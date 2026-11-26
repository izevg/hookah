// `orca plugin test .` runs this in Node. It covers the code that reads hookah's output and the
// client that runs it; no hookah binary is needed except for the two `exec` checks, which run Node.
import { assert, test } from 'orca/testing'
import {
  candidates,
  describe,
  exec,
  harnessFor,
  hookahClient,
  label,
  parseDetect,
  parseStatus,
  setupScope,
  summarize,
  type Exec
} from '../src/hookah'

const DETECT = [
  'claude: installed (binary /usr/bin/claude); project: not configured; user: not configured',
  'codex: not found; project: not configured; user: not configured',
  'qoder: installed (config directory found, no binary on PATH); project: configured; user: not configured',
  'kimi: installed (binary /usr/bin/kimi); project: no project config; user: not configured',
  'opencode: installed (binary /usr/bin/opencode); project: not configured; user: configured',
  'next: for each installed harness you use that is not configured, run `hookah setup <name>`.'
].join('\n')

const STATUS = JSON.stringify({
  v: 1,
  root: '/w',
  overall: 'fail',
  projects: [
    {
      root: '/w/a',
      status: { phase: 'stop', standing: 'fail', problems: 2, detail: null, at_ms: 20, lines: ['src/a.rs:1: boom', 'src/a.rs:2: bang'] }
    },
    { root: '/w/b', status: { phase: 'edit', standing: 'pass', problems: 0, detail: null, at_ms: 30, lines: [] } },
    { root: '/w/c', status: null }
  ]
})

test('parses the survey lines and skips the hint', () => {
  const found = parseDetect(DETECT)
  assert.deepEqual(
    found.map((d) => d.harness),
    ['claude', 'codex', 'qoder', 'kimi', 'opencode']
  )
  assert.deepEqual(found[0], { harness: 'claude', installed: true, project: 'missing', user: false })
  assert.equal(found[1].installed, false)
  assert.equal(found[3].project, 'none')
})

test('only an installed agent without hooks needs setup in its supported scope', () => {
  assert.deepEqual(parseDetect(DETECT).map(setupScope), ['project', null, null, 'user', null])
})

test('the standalone ZCode CLI needs user hooks regardless of project setup', () => {
  const zcode = { harness: 'zcode', installed: true, user: false }
  assert.equal(setupScope({ ...zcode, project: 'missing' }), 'user')
  assert.equal(setupScope({ ...zcode, project: 'configured' }), 'user')
  assert.equal(setupScope({ ...zcode, project: 'missing', user: true }), null)
  assert.equal(setupScope({ ...zcode, project: 'missing', installed: false }), null)
})

test('reads the status document and rejects anything else', () => {
  const rows = parseStatus(STATUS)
  assert.equal(rows.length, 3)
  assert.deepEqual(rows[0].status, {
    phase: 'stop',
    standing: 'fail',
    problems: 2,
    detail: null,
    atMs: 20,
    lines: ['src/a.rs:1: boom', 'src/a.rs:2: bang']
  })
  assert.equal(rows[2].status, null)
  assert.throws(() => parseStatus('{"v":2,"projects":[]}'))
  assert.throws(() => parseStatus('not json'))
})

test('summarizes to the worst standing, the problem total, and the newest time', () => {
  assert.deepEqual(summarize(parseStatus(STATUS)), { overall: 'fail', problems: 2, atMs: 30 })
  assert.deepEqual(summarize([{ root: '/x', status: null }]), { overall: null, problems: 0, atMs: null })
})

test('labels the status bar', () => {
  assert.equal(label({ overall: 'fail', problems: 1 }).text, 'hookah: 1 problem')
  assert.equal(label({ overall: 'fail', problems: 3 }).tone, 'danger')
  assert.equal(label({ overall: 'degraded', problems: 0 }).text, 'hookah: skipped')
  assert.equal(label({ overall: 'pass', problems: 0 }).tone, 'success')
  assert.equal(label({ overall: null, problems: 0 }).text, 'hookah')
})

test('describes a worktree in plain text', () => {
  assert.equal(describe('feat', parseStatus(STATUS)).split('\n')[0], '/w/a: 2 problems after the last stop check')
  assert.equal(describe('feat', []), 'feat: hookah has not checked anything here yet')
  const one = parseStatus(STATUS).slice(0, 1)
  assert.equal(describe('feat', one).split('\n')[2], '  src/a.rs:2: bang')
})

test('maps Orca agent names to hookah harnesses', () => {
  assert.equal(harnessFor('claude'), 'claude')
  assert.equal(harnessFor('antigravity'), 'antigravity')
  assert.equal(harnessFor('qoder-cn'), 'qoder')
  assert.equal(harnessFor('zcode'), 'zcode')
  assert.equal(harnessFor('gemini'), null)
})

test('tries the installer locations only for the default command', () => {
  assert.deepEqual(candidates('/opt/hookah', { HOME: '/h' }), ['/opt/hookah'])
  assert.deepEqual(candidates('hookah', { HOME: '/h' }), ['hookah', '/h/.local/bin/hookah'])
})

test('the client passes the documented arguments and reports failures', async () => {
  const calls: string[] = []
  const run: Exec = async (file, args, options) => {
    calls.push([file, ...args, options.cwd].join(' '))
    if (args[0] === 'status') return { code: 0, stdout: STATUS, stderr: '', failed: null }
    if (args[0] === 'check') return { code: 1, stdout: 'FAIL .\n', stderr: '', failed: null }
    return { code: 2, stdout: '', stderr: "error: unrecognized subcommand 'setup'\n", failed: null }
  }
  const hookah = hookahClient(run, 'hookah')
  assert.equal((await hookah.status('/w')).length, 3)
  assert.deepEqual(await hookah.check('/w', 1000), { passed: false, output: 'FAIL .\n' })
  await assert.rejects(hookah.detect('/w'), /too old/)
  assert.deepEqual(calls, ['hookah status --json --root /w /w', 'hookah check --changed --root /w /w', 'hookah setup --detect /w'])
})

test('a check that never ran is an error, a failed check is not', async () => {
  const gone: Exec = async () => ({ code: null, stdout: '', stderr: '', failed: 'spawn hookah ENOENT' })
  await assert.rejects(hookahClient(gone, 'hookah').check('/w', 1000), /ENOENT/)
  const broken: Exec = async () => ({ code: 3, stdout: '', stderr: 'boom\n', failed: null })
  await assert.rejects(hookahClient(broken, 'hookah').check('/w', 1000), /boom/)
})

test('exec reports an exit code, and a program that cannot start', async () => {
  const bad = await exec('node', ['-e', 'process.exit(3)'], { cwd: '.', timeoutMs: 20000 })
  assert.equal(bad.code, 3)
  assert.equal(bad.failed, null)
  const ok = await exec('node', ['-e', 'console.log("hi")'], { cwd: '.', timeoutMs: 20000 })
  assert.deepEqual([ok.code, ok.stdout.trim()], [0, 'hi'])
  const missing = await exec('hookah-no-such-program', [], { cwd: '.', timeoutMs: 5000 })
  assert.equal(missing.code, null)
  assert.notEqual(missing.failed, null)
})
