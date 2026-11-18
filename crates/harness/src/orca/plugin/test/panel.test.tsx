// Mounts src/panel.tsx over a mock worker.
import { assert, mockHost, mount, test } from 'orca/testing'
import * as panel from '../src/panel'
import type { WorktreeRow } from '../src/protocol'

const row = (over: Partial<WorktreeRow>): WorktreeRow => ({
  workspaceId: 'w1',
  name: 'feat',
  branch: 'feat',
  path: '/repo/feat',
  overall: 'fail',
  problems: 2,
  atMs: 1000,
  running: false,
  projects: [
    { root: '/repo/feat', status: { phase: 'stop', standing: 'fail', problems: 2, detail: null, atMs: 1000, lines: ['a.rs:1: x'] } }
  ],
  ...over
})

test('lists worktrees and shows the failing lines of the selected one', async () => {
  const worktrees = [row({}), row({ workspaceId: 'w2', name: 'fix', overall: null, problems: 0, atMs: null, projects: [] })]
  const started: string[] = []
  const host = mockHost({
    requests: {
      'hookah.list': () => ({ worktrees, error: null }),
      'hookah.check': ({ workspaceId }) => {
        started.push(workspaceId)
        return { started: true, error: null }
      }
    }
  })
  const view = await mount(panel, { host })
  assert.deepEqual(view.rowsOf({ type: 'Table' }), ['w1', 'w2'])

  await view.selectRows({ type: 'Table' }, ['w1'])
  assert.ok(view.find({ text: /a\.rs:1: x/ }))
  await view.press({ text: 'Check now' })
  assert.deepEqual(started, ['w1'])

  // The worker posts when something changed; the panel asks again.
  worktrees[0] = row({ overall: 'pass', problems: 0, projects: [] })
  await view.post('hookah.changed', { workspaceId: 'w1' })
  assert.equal(view.find({ text: /a\.rs:1: x/ }), null)
  assert.deepEqual(view.errors, [])
  assert.deepEqual(view.refusals, [])
  await view.unmount()
})

test('says why when hookah is unavailable', async () => {
  const host = mockHost({
    requests: { 'hookah.list': () => ({ worktrees: [], error: 'cannot run `hookah`' }) }
  })
  const view = await mount(panel, { host })
  assert.ok(view.find({ text: /cannot run `hookah`/ }))
  assert.deepEqual(view.errors, [])
  await view.unmount()
})
