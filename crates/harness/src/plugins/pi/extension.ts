/**
 * Pi extension for hookah: runs the project's own format, lint, and test checks on what the
 * agent edits. Install it with `hookah plugin pi > .pi/extensions/hookah.ts` (project) or into
 * `~/.pi/agent/extensions/` (global). See docs/setup/plugins/pi.md.
 *
 * Start: when a session starts (`session_start`) it runs `hookah hook pi` with `event: "start"`,
 * which records the problems the project already has (in the background) so later reports only
 * show what the agent introduced.
 *
 * Edit phase: after a file-writing tool (`write`, `edit`) it runs `hookah hook pi` and appends
 * hookah's report to the tool result the model reads.
 *
 * Stop phase: when the agent finishes a run (`agent_end`) after edits, it runs the checks over
 * the whole session. If they fail it injects the report once as a new message so the agent keeps
 * working. If the checks still fail after that injection, it only warns the user (no loop).
 *
 * The extension and the binary talk through one JSON document each way (contract version
 * `v: 1`). Everything fails open: a problem in the extension or in hookah never breaks a tool
 * call.
 *
 * Settings (environment): `HOOKAH_BIN` is the binary to run (default `hookah` from PATH). It may
 * be a wrapper script, for example one that runs `mise exec -- hookah "$@"`.
 */
import { spawn } from "node:child_process"
import type { ExtensionAPI, ExtensionContext } from "@mariozechner/pi-coding-agent"

const CONTRACT = 1
const EDIT_TIMEOUT_MS = 30_000
const STOP_TIMEOUT_MS = 120_000
const START_TIMEOUT_MS = 10_000
const IDLE_POLL_MS = 25
const IDLE_WAIT_MS = 10_000

/** hookah's answer: `block` means the model must act on `model_text`; `user_text` is for people. */
type Reply = { v?: number; block?: boolean; model_text?: string; user_text?: string }

/** Built-in tools whose `path` argument names the one file they write. */
const FILE_TOOLS = new Set(["write", "edit"])

/** The files a tool call leaves behind, as the model passed them (possibly relative). */
function editedFiles(tool: string, input: unknown): string[] {
  if (!FILE_TOOLS.has(tool) || !input || typeof input !== "object") return []
  const args = input as Record<string, unknown>
  const file = args.path ?? args.file_path
  return typeof file === "string" && file ? [file] : []
}

export default function (pi: ExtensionAPI) {
  /** True if the agent wrote files since the last stop check. */
  let dirty = false
  /** True if the run that just ended was started by a message this extension injected (the loop guard). */
  let continuing = false
  /** True while a stop check is in flight. */
  let checking = false

  /** True if the session has a UI. A stale context (the session ended) counts as none. */
  const hasUI = (ctx: ExtensionContext): boolean => {
    try {
      return ctx.hasUI
    } catch {
      return false
    }
  }

  /** Tells the user only (never the model): a notification, or stderr when there is no UI. */
  const warnUser = (ctx: ExtensionContext, text: string) => {
    try {
      if (hasUI(ctx)) ctx.ui.notify(text, "warning")
      else process.stderr.write(`${text}\n`)
    } catch {}
  }

  /** Active model id (`provider/id` when provider is known), or undefined. */
  const modelOf = (ctx: ExtensionContext): string | undefined => {
    try {
      const m = ctx.model as { provider?: string; id?: string } | undefined
      if (!m?.id) return undefined
      return m.provider && !m.id.includes("/") ? `${m.provider}/${m.id}` : m.id
    } catch {
      return undefined
    }
  }

  /** Runs `hookah hook pi`. Never throws; a failure becomes a user-facing warning. */
  const callHookah = (
    ctx: ExtensionContext,
    payload: Record<string, unknown>,
    timeoutMs: number,
  ): Promise<Reply | undefined> =>
    new Promise((resolve) => {
      let settled = false
      const done = (reply?: Reply, warning?: string) => {
        if (settled) return
        settled = true
        if (warning) warnUser(ctx, `hookah: ${warning}; skipping checks`)
        resolve(reply)
      }
      try {
        const bin = process.env.HOOKAH_BIN || "hookah"
        const model = modelOf(ctx)
        const child = spawn(bin, ["hook", "pi"], {
          cwd: ctx.cwd,
          env: process.env,
          stdio: ["pipe", "pipe", "pipe"],
          timeout: timeoutMs,
        })
        let out = ""
        let err = ""
        child.stdout.on("data", (d) => (out += d))
        child.stderr.on("data", (d) => (err += d))
        child.stdin.on("error", () => undefined)
        child.on("error", (e: NodeJS.ErrnoException) =>
          done(
            undefined,
            e.code === "ENOENT"
              ? `\`${bin}\` not found; install hookah or set HOOKAH_BIN`
              : `cannot run ${bin}: ${e.message}`,
          ),
        )
        child.on("close", (code, signal) => {
          if (signal) return done(undefined, `${bin} timed out after ${timeoutMs / 1000}s`)
          // hookah's own warnings (malformed payload, internal error) arrive on stderr.
          const warning = err.trim() || undefined
          if (code !== 0 || !out.trim()) return done(undefined, warning ?? `${bin} exited ${code}`)
          try {
            const reply = JSON.parse(out) as Reply
            done(reply.v === CONTRACT ? reply : undefined, reply.v === CONTRACT ? warning : "unknown reply version")
          } catch {
            done(undefined, "unreadable reply")
          }
        })
        child.stdin.end(JSON.stringify({ v: CONTRACT, cwd: ctx.cwd, ...(model ? { model } : {}), ...payload }))
      } catch (e) {
        done(undefined, `cannot run hookah: ${e}`)
      }
    })

  /**
   * Starts a new agent run with `text`. `agent_end` fires while the agent still counts as busy,
   * and a message queued then would wait for the user's next prompt, so this waits for idle.
   */
  const inject = async (ctx: ExtensionContext, text: string) => {
    const deadline = Date.now() + IDLE_WAIT_MS
    while (!ctx.isIdle()) {
      if (Date.now() > deadline) throw new Error("agent did not become idle")
      await new Promise((r) => setTimeout(r, IDLE_POLL_MS))
    }
    pi.sendMessage({ customType: "hookah", content: text, display: true }, { triggerTurn: true })
  }

  pi.on("session_start", async (event, ctx) => {
    try {
      if (event.reason === "reload") return
      dirty = false
      continuing = false
      const reply = await callHookah(
        ctx,
        { event: "start", session_id: ctx.sessionManager.getSessionId(), already_continuing: false },
        START_TIMEOUT_MS,
      )
      if (reply?.user_text) warnUser(ctx, reply.user_text)
    } catch {}
  })

  pi.on("tool_result", async (event, ctx) => {
    try {
      if (event.isError) return
      const files = editedFiles(event.toolName, event.input)
      if (files.length === 0) return
      dirty = true
      const reply = await callHookah(
        ctx,
        { event: "edit", files, session_id: ctx.sessionManager.getSessionId(), already_continuing: false },
        EDIT_TIMEOUT_MS,
      )
      if (!reply) return
      if (reply.user_text) warnUser(ctx, reply.user_text)
      if (reply.model_text) {
        return { content: [...event.content, { type: "text" as const, text: `\n${reply.model_text}` }] }
      }
    } catch {}
  })

  pi.on("agent_end", async (event, ctx) => {
    const injected = continuing
    const interactive = hasUI(ctx)
    if (checking || (!dirty && !injected)) return
    // A run the user cancelled or that errored out is not a finished piece of work: check later.
    const last = [...event.messages].reverse().find((m) => m.role === "assistant") as
      | { stopReason?: string }
      | undefined
    if (last?.stopReason === "aborted" || last?.stopReason === "error") return
    checking = true
    try {
      dirty = false
      continuing = false
      const reply = await callHookah(
        ctx,
        { event: "stop", session_id: ctx.sessionManager.getSessionId(), already_continuing: injected },
        STOP_TIMEOUT_MS,
      )
      if (!reply) return
      if (reply.user_text) warnUser(ctx, reply.user_text)
      if (reply.block && reply.model_text) {
        // Remember the injection first: the run it starts ends in another agent_end.
        continuing = true
        // Print and JSON mode exit once the run ends, before an injected run could start: show
        // the report on stderr instead.
        if (!interactive) warnUser(ctx, reply.model_text)
        inject(ctx, reply.model_text).catch((e) => {
          continuing = false
          if (interactive) warnUser(ctx, `hookah: cannot continue the agent with the report: ${e}`)
        })
      }
    } catch {
    } finally {
      checking = false
    }
  })
}
