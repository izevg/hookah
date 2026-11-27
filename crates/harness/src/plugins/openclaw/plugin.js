/**
 * OpenClaw plugin for hookah: runs the project's own format, lint, and test checks on what the
 * agent edits. Install it with `hookah setup openclaw` (or print it with `hookah plugin openclaw`).
 * See docs/setup/assistants/openclaw.md.
 *
 * Start: when a session starts it runs `hookah hook openclaw` with `event: "start"`, which
 * records the problems the project already has (in the background) so later reports only show
 * what the agent introduced.
 *
 * Edit phase: after the file tools `write`, `edit`, and `apply_patch` it runs `hookah hook
 * openclaw` and appends hookah's report to the tool result the model reads next (tool-result middleware).
 *
 * Stop phase: when the agent is about to give its final answer after edits, it runs the checks
 * over the whole session (`before_agent_finalize`). If they fail it hands the model the report
 * once: as steering for the active run when the host accepts it, else queued for the next turn
 * (the host's embedded runner never reruns a turn that edited files), and as a revision request
 * for runtimes that honor one. If the checks still fail after that, it only warns the user.
 *
 * The plugin and the binary talk through one JSON document each way (contract version `v: 1`).
 * Everything fails open: a problem in the plugin or in hookah never breaks a tool call or a turn.
 *
 * Settings (environment): `HOOKAH_BIN` is the binary to run (default `hookah` from PATH).
 */
import { execFile } from "node:child_process"
import path from "node:path"

const CONTRACT = 1
const EDIT_TIMEOUT_MS = 30_000
const STOP_TIMEOUT_MS = 120_000
const START_TIMEOUT_MS = 10_000
const NEXT_TURN_TTL_MS = 24 * 60 * 60 * 1000
const FILE_TOOLS = new Set(["write", "edit"])

/** Added, updated, and moved-to files of an `apply_patch` text; deletions are skipped. */
function patchedFiles(patch) {
  const files = []
  for (const line of patch.split("\n")) {
    const add = /^\*\*\* (?:Add|Update) File:\s*(.+?)\s*$/.exec(line)
    const move = /^\*\*\* Move to:\s*(.+?)\s*$/.exec(line)
    if (add) files.push(add[1])
    else if (move && files.length > 0) files[files.length - 1] = move[1]
  }
  return [...new Set(files)]
}

/** The files a tool call leaves behind, as the model passed them (possibly relative). */
function editedFiles(tool, params) {
  if (!params || typeof params !== "object") return []
  if (FILE_TOOLS.has(tool)) {
    const file = params.path ?? params.file_path ?? params.filePath
    return typeof file === "string" && file ? [file] : []
  }
  if (tool === "apply_patch") {
    const patch = params.input ?? params.patch
    return typeof patch === "string" ? patchedFiles(patch) : []
  }
  return []
}

export default {
  id: "hookah",
  name: "hookah",
  description: "Runs the project's own checks after file edits and feeds failures back.",
  register(api) {
    // The host may load the plugin more than once (one registry per runtime phase), so the
    // state lives on `globalThis`, shared by every copy.
    const state = (globalThis[Symbol.for("hookah.openclaw")] ??= {
      /** Workspace directory by session key, learned from the agent-context hooks. */
      workspaces: new Map(),
      /** Latest model identifier by session key, learned from model hooks. */
      models: new Map(),
      /** Session keys that edited files since their last stop check. */
      dirty: new Set(),
      /** Session keys whose last stop check asked for one more pass (the loop guard). */
      continuing: new Set(),
      lastWorkspace: undefined,
      lastModel: undefined,
    })
    state.models ??= new Map()
    const { workspaces, models, dirty, continuing } = state

    const warn = (text) => {
      try {
        api.logger?.warn?.(`hookah: ${text}`)
      } catch {}
    }

    /** Runs `hookah hook openclaw`. Never throws; a failure becomes a warning for the user. */
    const callHookah = (cwd, payload, timeout) =>
      new Promise((resolve) => {
        const bin = process.env.HOOKAH_BIN || "hookah"
        const fail = (why) => {
          warn(`${why}; skipping checks`)
          resolve(undefined)
        }
        try {
          const child = execFile(bin, ["hook", "openclaw"], { cwd, env: process.env, timeout, encoding: "utf8" }, (e, out) => {
            if (e) return fail(e.code === "ENOENT" ? `\`${bin}\` not found; install hookah or set HOOKAH_BIN` : e.message)
            try {
              const reply = JSON.parse(out)
              if (reply.v !== CONTRACT) return fail("unknown reply version")
              if (reply.user_text) warn(reply.user_text)
              resolve(reply)
            } catch {
              fail("unreadable reply")
            }
          })
          child.stdin?.on("error", () => undefined)
          child.stdin?.end(JSON.stringify({ v: CONTRACT, cwd, ...payload }))
        } catch (e) {
          fail(`cannot run hookah: ${e}`)
        }
      })

    const sessionOf = (ctx) => ctx?.sessionKey || ctx?.sessionId
    const dirOf = (ctx) => ctx?.workspaceDir || workspaces.get(sessionOf(ctx)) || state.lastWorkspace || process.cwd()
    const formatModel = (provider, model) => {
      const id = typeof model === "string" ? model : model?.id
      if (typeof id !== "string" || !id) return undefined
      const prov = typeof provider === "string" ? provider : model?.provider
      return typeof prov === "string" && prov && !id.includes("/") ? `${prov}/${id}` : id
    }
    const rememberModel = (session, provider, model) => {
      const name = formatModel(provider, model)
      if (!name) return
      state.lastModel = name
      if (session) {
        if (models.size > 64) models.clear()
        models.set(session, name)
      }
    }
    const modelOf = (event, ctx) =>
      formatModel(event?.provider ?? ctx?.provider, event?.model ?? ctx?.model) ||
      models.get(sessionOf(ctx) || event?.sessionKey || event?.sessionId) ||
      state.lastModel

    api.on("before_model_resolve", (event, ctx) => {
      const session = sessionOf(ctx)
      rememberModel(session, event?.provider ?? ctx?.provider, event?.model ?? ctx?.model)
      if (!ctx?.workspaceDir) return
      state.lastWorkspace = ctx.workspaceDir
      if (session) workspaces.set(session, ctx.workspaceDir)
    })

    api.on("llm_input", (event, ctx) => {
      rememberModel(sessionOf(ctx) || event?.sessionKey || event?.sessionId, event?.provider ?? ctx?.provider, event?.model ?? ctx?.model)
    })

    api.on("session_start", async (event, ctx) => {
      try {
        const id = event.sessionKey || event.sessionId
        if (!id) return
        const model = modelOf(event, ctx || { sessionKey: id })
        await callHookah(
          dirOf({ sessionKey: id }),
          { event: "start", session_id: id, already_continuing: false, ...(model ? { model } : {}) },
          START_TIMEOUT_MS,
        )
      } catch (e) {
        warn(`session start failed: ${e}`)
      }
    })

    // The middleware runs before the runtime feeds the tool result back to the model, which is
    // the only point where text can reach the model right after an edit.
    api.registerAgentToolResultMiddleware(
      async (event, ctx) => {
        try {
          const files = editedFiles(event.toolName, event.args)
          if (!files.length || event.isError) return
          const cwd = event.cwd || dirOf(ctx)
          const session = sessionOf(ctx)
          const model = modelOf(event, ctx)
          dirty.add(session)
          const reply = await callHookah(
            cwd,
            {
              event: "edit",
              files: files.map((f) => path.resolve(cwd, f)),
              session_id: session,
              already_continuing: false,
              ...(model ? { model } : {}),
            },
            EDIT_TIMEOUT_MS,
          )
          if (!reply?.model_text) return
          const content = Array.isArray(event.result?.content) ? event.result.content : []
          return { result: { ...event.result, content: [...content, { type: "text", text: reply.model_text }] } }
        } catch (e) {
          warn(`edit check failed: ${e}`)
        }
      },
      { runtimes: ["openclaw"], matcher: ["write", "edit", "apply_patch"] },
    )

    api.on("before_agent_finalize", async (event, ctx) => {
      try {
        const id = sessionOf(ctx) || event.sessionKey || event.sessionId
        const injected = continuing.has(id)
        if (!dirty.has(id) && !injected) return
        dirty.delete(id)
        continuing.delete(id)
        const model = modelOf(event, ctx)
        const reply = await callHookah(
          event.cwd || dirOf(ctx),
          { event: "stop", session_id: id, already_continuing: injected, ...(model ? { model } : {}) },
          STOP_TIMEOUT_MS,
        )
        if (reply?.block && reply.model_text) {
          continuing.add(id)
          const sessionKey = event.sessionKey || ctx?.sessionKey
          const guidance = { sessionKey, text: reply.model_text, idempotencyKey: `hookah-${event.runId || id}` }
          const workflow = api.session?.workflow
          const steered = sessionKey ? await workflow?.steerActiveRun?.(guidance).catch(() => undefined) : undefined
          if (steered?.steered) return
          const queued = sessionKey
            ? await workflow?.enqueueNextTurnInjection?.({ ...guidance, ttlMs: NEXT_TURN_TTL_MS }).catch(() => undefined)
            : undefined
          if (queued?.enqueued) warn("checks are failing; the report is queued for the agent's next turn")
          return { action: "revise", reason: reply.model_text }
        }
      } catch (e) {
        warn(`stop check failed: ${e}`)
      }
    })
  },
}
