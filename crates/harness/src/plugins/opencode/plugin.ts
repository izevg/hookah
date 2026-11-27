/**
 * opencode plugin for hookah: runs the project's own format, lint, and test checks on what the
 * agent edits. Install it with `hookah plugin opencode > .opencode/plugins/hookah.ts` (project)
 * or into `~/.config/opencode/plugins/` (global). See docs/setup/plugins/opencode.md.
 *
 * Start: when a session is created it runs `hookah hook opencode` with `event: "start"`, which
 * records the problems the project already has (in the background) so later reports only show
 * what the agent introduced.
 *
 * Edit phase: after a file-writing tool (`write`, `edit`, `multiedit`, `apply_patch`) it runs
 * `hookah hook opencode` and appends hookah's report to the tool output the model reads.
 *
 * Stop phase: when a session goes idle after edits, it runs the checks over the whole session.
 * If they fail it injects the report once as a new user message so the agent keeps working. If
 * the checks still fail after that injection, it only warns the user (no loop).
 *
 * The plugin and the binary talk through one JSON document each way (contract version `v: 1`).
 * Everything fails open: a problem in the plugin or in hookah never breaks a tool call.
 *
 * Settings (environment): `HOOKAH_BIN` is the binary to run (default `hookah` from PATH). It may
 * be a wrapper script, for example one that runs `mise exec -- hookah "$@"`.
 */
import { spawn } from "node:child_process"
import type { Plugin } from "@opencode-ai/plugin"

const CONTRACT = 1
const EDIT_TIMEOUT_MS = 30_000
const STOP_TIMEOUT_MS = 120_000
const START_TIMEOUT_MS = 10_000
const MAX_TOAST = 600

/** hookah's answer: `block` means the model must act on `model_text`; `user_text` is for people. */
type Reply = { v?: number; block?: boolean; model_text?: string; user_text?: string }

/** Tools whose `filePath` argument names the one file they write. */
const FILE_TOOLS = new Set(["write", "edit", "multiedit"])

/** The files a tool call leaves behind, as the model passed them (possibly relative). */
function editedFiles(tool: string, args: any): string[] {
  if (!args || typeof args !== "object") return []
  if (FILE_TOOLS.has(tool)) {
    const file = args.filePath ?? args.path
    return typeof file === "string" && file ? [file] : []
  }
  if (tool === "apply_patch" || tool === "patch") {
    return typeof args.patchText === "string" ? patchedFiles(args.patchText) : []
  }
  return []
}

/** Added, updated, and moved-to files of an `apply_patch` text; deletions are skipped. */
function patchedFiles(patch: string): string[] {
  const files: string[] = []
  for (const line of patch.split("\n")) {
    const add = /^\*\*\* (?:Add|Update) File:\s*(.+?)\s*$/.exec(line)
    const move = /^\*\*\* Move to:\s*(.+?)\s*$/.exec(line)
    if (add) files.push(add[1])
    else if (move && files.length > 0) files[files.length - 1] = move[1]
  }
  return [...new Set(files)]
}

/** Composes `provider/model` when provider is known, preserving any `/` inside routed model IDs. */
function formatModel(provider: unknown, model: unknown): string | undefined {
  if (typeof model !== "string" || !model) return undefined
  if (typeof provider !== "string" || !provider) return model
  const prefix = `${provider}/`
  return model.toLowerCase().startsWith(prefix.toLowerCase()) ? model : `${prefix}${model}`
}

export const HookahPlugin: Plugin = async ({ client, directory }) => {
  /** Arguments seen in `tool.execute.before`, used when `after` does not carry them. */
  const pendingArgs = new Map<string, unknown>()
  /** Latest model seen per session, learned from `chat.params` and `message.updated`. */
  const models = new Map<string, string>()
  /** Top-level sessions awaiting their first model-bearing event before starting the baseline. */
  const pendingStart = new Set<string>()
  /** Sessions that edited files since their last stop check. */
  const dirty = new Set<string>()
  /** Sessions whose latest turn was started by a message this plugin injected (the loop guard). */
  const continuing = new Set<string>()
  /** Sessions with a stop check in flight. */
  const checking = new Set<string>()

  const startBaseline = async (sessionID: string, model?: string) => {
    try {
      const reply = await callHookah(
        {
          event: "start",
          session_id: sessionID,
          already_continuing: false,
          ...(model ? { model } : {}),
        },
        START_TIMEOUT_MS,
      )
      if (reply?.user_text) await warnUser(reply.user_text)
    } catch (e) {
      await log("warn", `hookah session start failed: ${e}`)
    }
  }

  const rememberModel = async (sessionID: unknown, provider: unknown, model: unknown) => {
    const name = formatModel(provider, model)
    if (typeof sessionID === "string" && sessionID && name) {
      if (models.size > 64) models.clear()
      models.set(sessionID, name)
      if (pendingStart.delete(sessionID)) {
        await startBaseline(sessionID, name)
      }
    }
  }

  const log = (level: "info" | "warn", message: string) =>
    client.app
      .log({ body: { service: "hookah", level, message } })
      .catch(() => undefined)

  /** Tells the user only (never the model): a toast, plus a log line for the record. */
  const warnUser = async (text: string) => {
    const message = text.length > MAX_TOAST ? `${text.slice(0, MAX_TOAST)}...` : text
    await client.tui
      .showToast({ body: { title: "hookah", message, variant: "warning", duration: 8000 } })
      .catch(() => undefined)
    await log("warn", text)
  }

  /** Runs `hookah hook opencode`. Never throws; a failure becomes a user-facing warning. */
  const callHookah = (
    payload: Record<string, unknown>,
    timeoutMs: number,
  ): Promise<Reply | undefined> =>
    new Promise((resolve) => {
      let settled = false
      const done = (reply?: Reply, warning?: string) => {
        if (settled) return
        settled = true
        if (warning) void warnUser(`hookah: ${warning}; skipping checks`)
        resolve(reply)
      }
      try {
        const bin = process.env.HOOKAH_BIN || "hookah"
        const child = spawn(bin, ["hook", "opencode"], {
          cwd: directory,
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
        child.stdin.end(JSON.stringify({ v: CONTRACT, cwd: directory, ...payload }))
      } catch (e) {
        done(undefined, `cannot run hookah: ${e}`)
      }
    })

  return {
    "chat.params": async (input) => {
      try {
        const raw = input as any
        const modelId = typeof raw?.model === "string" ? raw.model : raw?.model?.id
        const providerId =
          (typeof raw?.model === "object" ? raw?.model?.providerID : undefined) ?? raw?.provider?.id
        await rememberModel(raw?.sessionID, providerId, modelId)
      } catch {}
    },

    "tool.execute.before": async (input, output) => {
      try {
        if (!editedFiles(input.tool, output.args).length) return
        if (pendingArgs.size > 64) pendingArgs.clear()
        pendingArgs.set(input.callID, output.args)
      } catch {}
    },

    "tool.execute.after": async (input, output) => {
      try {
        const args = input.args ?? pendingArgs.get(input.callID)
        pendingArgs.delete(input.callID)
        const files = editedFiles(input.tool, args)
        if (files.length === 0) return
        pendingStart.delete(input.sessionID)
        dirty.add(input.sessionID)
        const model = models.get(input.sessionID)
        const reply = await callHookah(
          {
            event: "edit",
            files,
            session_id: input.sessionID,
            already_continuing: false,
            ...(model ? { model } : {}),
          },
          EDIT_TIMEOUT_MS,
        )
        if (!reply) return
        if (reply.model_text && typeof output.output === "string") {
          output.output = `${output.output}\n\n${reply.model_text}`
        }
        if (reply.user_text) await warnUser(reply.user_text)
      } catch (e) {
        await log("warn", `hookah edit check failed: ${e}`)
      }
    },

    event: async ({ event }) => {
      if (event.type === "message.updated") {
        try {
          const info = (event.properties as any)?.info
          await rememberModel(
            info?.sessionID,
            info?.providerID ?? info?.model?.providerID,
            info?.modelID ?? info?.model?.modelID,
          )
        } catch {}
        return
      }
      if (event.type === "session.created") {
        try {
          const info = event.properties.info
          // Subagent sessions work on the parent's project: they get a baseline lazily if needed.
          if (!info?.id || info.parentID) return
          const model = models.get(info.id)
          if (model) {
            await startBaseline(info.id, model)
          } else {
            if (pendingStart.size > 64) pendingStart.clear()
            pendingStart.add(info.id)
          }
        } catch (e) {
          await log("warn", `hookah session start failed: ${e}`)
        }
        return
      }
      if (event.type !== "session.idle") return
      const id = event.properties.sessionID
      const injected = continuing.has(id)
      if (checking.has(id) || (!dirty.has(id) && !injected)) return
      checking.add(id)
      try {
        dirty.delete(id)
        continuing.delete(id)
        const model = models.get(id)
        const reply = await callHookah(
          {
            event: "stop",
            session_id: id,
            already_continuing: injected,
            ...(model ? { model } : {}),
          },
          STOP_TIMEOUT_MS,
        )
        if (!reply) return
        if (reply.user_text) await warnUser(reply.user_text)
        if (reply.block && reply.model_text) {
          // Remember the injection first: the turn it starts ends in another session.idle.
          continuing.add(id)
          try {
            await client.session.promptAsync({
              path: { id },
              body: { parts: [{ type: "text", text: reply.model_text }] },
            })
          } catch (e) {
            continuing.delete(id)
            throw e
          }
        }
      } catch (e) {
        await log("warn", `hookah stop check failed: ${e}`)
      } finally {
        checking.delete(id)
      }
    },
  }
}
