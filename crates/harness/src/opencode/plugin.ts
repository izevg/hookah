/**
 * opencode plugin shim for hookah.
 *
 * Installed into the user's opencode plugin directory by following the setup docs. It
 * subscribes to `tool.execute.after` for file-writing tools, runs `hookah hook opencode` with
 * the event as JSON on stdin, and appends the returned report to the tool output. When the
 * agent goes idle, it runs the stop phase the same way.
 *
 * Embedded into the binary with `include_str!` so `hookah` can print it during setup.
 */
