"""hookah plugin for Hermes Agent: runs the project's own format, lint, and test checks on what
the agent edits. Install it with `hookah setup hermes`, or by hand into
`~/.hermes/plugins/hookah/` (this file as `__init__.py`, plus a `plugin.yaml`) and enable it with
`hermes plugins enable hookah`. See docs/setup/assistants/hermes.md.

Start: when a new session begins it runs `hookah hook hermes` with `event: "start"`, which records
the problems the project already has (in the background) so later reports only show what the
agent introduced.

Edit phase: after a file-writing tool (`write_file`, `patch`) it runs `hookah hook hermes` and
appends hookah's report to the tool result the model reads (`transform_tool_result`).

Stop phase: when the agent is about to finish a turn in which it edited files (`pre_verify`), it
runs the checks over the whole session. If they fail, it asks the agent to continue once with the
report. If the checks still fail on the next attempt, it only warns the user (no loop).

The plugin and the binary talk through one JSON document each way (contract version `v: 1`).
Everything fails open: a problem in the plugin or in hookah never breaks a tool call or a turn.

Settings (environment): `HOOKAH_BIN` is the binary to run (default `hookah` from PATH). It may
be a wrapper script, for example one that runs `mise exec -- hookah "$@"`. Hermes abandons a
plugin hook that runs longer than `plugins.hook_callback_timeout` (30 s by default); raise it in
`config.yaml` if the stop checks run slow tests.
"""

import json
import logging
import os
import re
import subprocess

CONTRACT = 1
EDIT_TIMEOUT_S = 30
STOP_TIMEOUT_S = 120
START_TIMEOUT_S = 10
MAX_SESSIONS = 256
MAX_IN_FLIGHT = 4096

log = logging.getLogger("hookah")


def _warn(text):
    """Tells the user only (never the model): a line in Hermes's own log."""
    log.warning("%s", text if text.startswith("hookah:") else "hookah: " + text)


def _cwd(task_id=""):
    """The directory the agent stands in: the base Hermes resolves the session's relative paths
    against (the gateway process that runs the agent stands elsewhere), else the terminal's
    working directory, else the process's."""
    try:
        from tools.file_tools_paths import _resolve_base_dir

        cwd = str(_resolve_base_dir(task_id or "default"))
        if os.path.isabs(cwd) and os.path.isdir(cwd):
            return cwd
    except Exception:
        pass
    return os.environ.get("TERMINAL_CWD") or os.getcwd()


def _call(payload, timeout, task_id=""):
    """Runs `hookah hook hermes`. Never raises; a failure becomes a warning and `None`."""
    binary = os.environ.get("HOOKAH_BIN") or "hookah"
    try:
        body = json.dumps({"v": CONTRACT, "cwd": _cwd(task_id), **payload})
        done = subprocess.run(
            [binary, "hook", "hermes"],
            input=body,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=timeout,
        )
    except FileNotFoundError:
        _warn("`%s` not found; install hookah or set HOOKAH_BIN; skipping checks" % binary)
        return None
    except subprocess.TimeoutExpired:
        _warn("%s timed out after %ss; skipping checks" % (binary, timeout))
        return None
    except Exception as e:
        _warn("cannot run %s: %s; skipping checks" % (binary, e))
        return None
    # hookah's own warnings (malformed payload, internal error) arrive on stderr.
    warning = done.stderr.strip()
    if done.returncode != 0 or not done.stdout.strip():
        _warn("%s; skipping checks" % (warning or "%s exited %s" % (binary, done.returncode)))
        return None
    try:
        reply = json.loads(done.stdout)
    except ValueError:
        _warn("unreadable reply; skipping checks")
        return None
    if not isinstance(reply, dict) or reply.get("v") != CONTRACT:
        _warn("unknown reply version; skipping checks")
        return None
    if warning:
        _warn(warning)
    return reply


_HEADER = re.compile(r"^\*\*\*\s*(Update|Add)\s+File:\s*(.+?)\s*$", re.MULTILINE)
_MOVE = re.compile(r"^\*\*\*\s*Move\s+File:\s*.+?\s*->\s*(.+?)\s*$", re.MULTILINE)


def _patched_files(patch):
    """Added, updated, and moved-to files of a V4A patch; deletions are skipped."""
    files = [m.group(2) for m in _HEADER.finditer(patch)]
    files.extend(m.group(1) for m in _MOVE.finditer(patch))
    return list(dict.fromkeys(files))


def _edited_files(tool, args, result):
    """The files a successful write tool call left behind, as Hermes reports them."""
    if tool not in ("write_file", "patch") or not isinstance(args, dict):
        return []
    try:
        data = json.loads(result) if isinstance(result, str) else None
    except ValueError:
        data = None
    if isinstance(data, dict):
        if data.get("error") or data.get("success") is False:
            return []
        landed = data.get("files_modified")
        if isinstance(landed, list) and landed:
            return [str(p) for p in landed if p]
        if data.get("resolved_path"):
            return [str(data["resolved_path"])]
    if tool == "patch" and (args.get("mode") or "replace") == "patch":
        patch = args.get("patch")
        return _patched_files(patch) if isinstance(patch, str) else []
    path = args.get("path")
    return [path] if isinstance(path, str) and path else []


def _with_report(result, text):
    """`result` with hookah's report in it. Hermes' own bookkeeping parses a file tool's result
    as JSON (it decides from it whether the write landed), so the report goes in as a field of
    that JSON, never as loose text after it."""
    try:
        data = json.loads(result)
    except ValueError:
        data = None
    if isinstance(data, dict):
        data["hookah"] = text
        return json.dumps(data, ensure_ascii=False)
    return "%s\n\n%s" % (result, text)


def register(ctx):
    models = {}
    in_flight = {}
    dirty = {}

    def _touch(table, sid):
        if not sid:
            return
        table.pop(sid, None)
        table[sid] = True
        while len(table) > MAX_IN_FLIGHT:
            victim = next(iter(table))
            table.pop(victim, None)
            if table is in_flight and victim not in dirty:
                models.pop(victim, None)

    def _evict_models():
        while len(models) > MAX_SESSIONS:
            victim = next((sid for sid in models if sid not in in_flight and sid not in dirty), None)
            if victim is not None:
                models.pop(victim, None)
                continue
            if len(models) <= MAX_IN_FLIGHT:
                break
            victim = next((sid for sid in models if sid not in dirty), None)
            if victim is None:
                victim = next(iter(models))
            models.pop(victim, None)
            in_flight.pop(victim, None)
            dirty.pop(victim, None)

    def _model_for(session_id, kwargs):
        raw = kwargs.get("model")
        if isinstance(raw, str) and raw.strip():
            model = raw.strip()
            if session_id:
                models.pop(session_id, None)
                models[session_id] = model
                _evict_models()
            return model
        if session_id and session_id in models:
            model = models.pop(session_id)
            models[session_id] = model
            return model
        return None

    def on_session_end(session_id="", task_id="", **_kwargs):
        sid = session_id or task_id or ""
        if sid:
            models.pop(sid, None)
            in_flight.pop(sid, None)
            dirty.pop(sid, None)

    def on_llm_call(session_id="", task_id="", **kwargs):
        try:
            sid = session_id or task_id or ""
            _touch(in_flight, sid)
            _model_for(sid, kwargs)
        except Exception:
            pass

    def on_session_start(session_id="", **kwargs):
        try:
            if session_id:
                payload = {"event": "start", "session_id": session_id}
                model = _model_for(session_id, kwargs)
                if model:
                    payload["model"] = model
                _call(payload, START_TIMEOUT_S, session_id)
        except Exception as e:
            _warn("session start failed: %s" % e)

    def on_result(tool_name="", args=None, result=None, session_id="", task_id="", **kwargs):
        try:
            files = _edited_files(tool_name, args, result)
            if not files:
                return None
            sid = session_id or task_id or ""
            _touch(dirty, sid)
            _touch(in_flight, sid)
            payload = {"event": "edit", "files": files, "session_id": sid or None}
            model = _model_for(sid, kwargs)
            if model:
                payload["model"] = model
            reply = _call(
                payload,
                EDIT_TIMEOUT_S,
                task_id or session_id,
            )
            if not reply:
                return None
            if reply.get("user_text"):
                _warn(reply["user_text"])
            text = reply.get("model_text")
            if text and isinstance(result, str):
                return _with_report(result, text)
        except Exception as e:
            _warn("edit check failed: %s" % e)
        return None

    def on_verify(session_id="", attempt=0, **kwargs):
        try:
            payload = {
                "event": "stop",
                "session_id": session_id or None,
                "already_continuing": bool(attempt),
            }
            model = _model_for(session_id, kwargs)
            if model:
                payload["model"] = model
            reply = _call(
                payload,
                STOP_TIMEOUT_S,
                session_id,
            )
            if not reply:
                return None
            if reply.get("user_text"):
                _warn(reply["user_text"])
            if reply.get("block") and reply.get("model_text"):
                return {"action": "continue", "message": reply["model_text"]}
            if session_id:
                dirty.pop(session_id, None)
                in_flight.pop(session_id, None)
                _evict_models()
        except Exception as e:
            _warn("stop check failed: %s" % e)
        return None

    ctx.register_hook("on_session_start", on_session_start)
    try:
        ctx.register_hook("on_session_end", on_session_end)
    except Exception:
        pass
    try:
        ctx.register_hook("pre_llm_call", on_llm_call)
    except Exception:
        pass
    ctx.register_hook("transform_tool_result", on_result)
    ctx.register_hook("pre_verify", on_verify)
