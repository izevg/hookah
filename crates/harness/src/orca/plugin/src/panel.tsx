// The sidebar panel: hookah's latest verdict per worktree. No build step: Orca compiles it as
// written. State lives in atoms; the worker (src/worker.ts) reads hookah and pushes changes.
import {
  atom,
  Badge,
  Button,
  Callout,
  Code,
  EmptyState,
  Heading,
  plugin,
  read,
  Row,
  set,
  Stack,
  Table,
  Text,
  type PanelContext
} from 'orca'
import type { Standing, WorktreeRow } from './protocol'

const rows = atom<WorktreeRow[] | null>(null)
const error = atom<string | null>(null)
const selected = atom<string | null>(null)

async function refresh(): Promise<void> {
  try {
    const answer = await plugin.request('hookah.list')
    set(rows, answer.worktrees)
    set(error, answer.error)
  } catch (e) {
    set(error, e instanceof Error ? e.message : String(e))
  }
}

async function check(workspaceId: string): Promise<void> {
  try {
    const answer = await plugin.request('hookah.check', { workspaceId })
    if (answer.error !== null) set(error, answer.error)
  } catch (e) {
    set(error, e instanceof Error ? e.message : String(e))
  }
}

export function setup(ctx: PanelContext): void {
  ctx.subscriptions.push(plugin.on('hookah.changed', () => void refresh()))
  void refresh()
}

const RESULT: Record<Standing, string> = { pass: 'ok', degraded: 'skipped', fail: 'failing' }

function result(w: WorktreeRow): string {
  if (w.running) return 'checking...'
  return w.overall === null ? 'not checked' : RESULT[w.overall]
}

function tone(w: WorktreeRow): 'success' | 'warning' | 'danger' | 'neutral' {
  if (w.overall === 'fail') return 'danger'
  if (w.overall === 'degraded') return 'warning'
  return w.overall === 'pass' ? 'success' : 'neutral'
}

export default function Panel() {
  const list = read(rows)
  const problem = read(error)
  const pick = read(selected)
  const current = list?.find((w) => w.workspaceId === pick) ?? null
  return (
    <Stack gap="md">
      <Row justify="between">
        <Heading>hookah</Heading>
        <Button variant="ghost" icon="refresh" onPress={refresh}>
          Refresh
        </Button>
      </Row>
      {problem !== null && (
        <Callout tone="warning" title="hookah is unavailable">
          {problem}
        </Callout>
      )}
      {list !== null && list.length === 0 && problem === null && (
        <EmptyState title="No worktrees" description="Create a workspace and let an agent edit a file." icon="branch" />
      )}
      {list !== null && list.length > 0 && (
        <Table
          label="Worktrees"
          selectionMode="single"
          selected={pick === null ? [] : [pick]}
          onSelectionChange={({ keys }) => set(selected, keys[0] ?? null)}
          emptyText="No worktrees"
          columns={[
            { key: 'name', label: 'Worktree' },
            { key: 'result', label: 'Result', width: 'sm' },
            { key: 'problems', label: 'Problems', align: 'end', width: 'xs', hideBelow: 'sm' },
            { key: 'when', label: 'Checked', width: 'sm', hideBelow: 'md' }
          ]}
          rows={list.map((w) => ({
            key: w.workspaceId,
            cells: {
              name: w.name,
              result: result(w),
              problems: w.overall === null ? '-' : w.problems,
              when: w.atMs === null ? '-' : { type: 'relative', at: w.atMs, format: 'short' }
            }
          }))}
        />
      )}
      {current !== null && (
        <Stack gap="sm">
          <Row justify="between">
            <Row gap="xs">
              <Text weight="semibold">{current.name}</Text>
              <Badge tone={tone(current)}>{result(current)}</Badge>
            </Row>
            <Button size="sm" icon="play" disabled={current.running} onPress={() => check(current.workspaceId)}>
              Check now
            </Button>
          </Row>
          <Text muted size="xs">
            {current.path}
          </Text>
          {current.projects.map((p) =>
            p.status === null ? (
              <Text key={p.root} muted size="xs">
                {p.root}: no run recorded
              </Text>
            ) : (
              <Stack key={p.root} gap="xs">
                <Text size="xs">
                  {p.root}: {p.status.standing === 'degraded' ? `skipped${p.status.detail === null ? '' : ` (${p.status.detail})`}` : `${p.status.problems} problem(s)`} after the last {p.status.phase} check
                </Text>
                {p.status.lines.length > 0 && <Code block>{p.status.lines.join('\n')}</Code>}
              </Stack>
            )
          )}
        </Stack>
      )}
    </Stack>
  )
}
