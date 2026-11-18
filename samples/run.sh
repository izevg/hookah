#!/usr/bin/env bash
# Runs every hookah sample scenario and diffs the output against `expected.out`.
# Usage: samples/run.sh [--update] [<filter>]
# See samples/README.md for the layout and the contract.
set -u

here=$(cd "$(dirname "$0")" && pwd -P)
repo=$(dirname "$here")
mise=${MISE:-$HOME/.local/bin/mise}
update=0
filter=""

for arg in "$@"; do
  case "$arg" in
    --update) update=1 ;;
    -h | --help)
      sed -n '2,3p' "$0"
      exit 0
      ;;
    *) filter=$arg ;;
  esac
done

# Git Bash (MSYS) on Windows: paths in the shell are POSIX style (`/c/Users/x`), but hookah and the
# toolchains are native programs, and payloads must carry native paths (`C:\Users\x`) like the
# ones Claude Code on Windows sends. `cygpath` translates; `-l` expands 8.3 short names such as
# `RUNNER~1`, which hookah never prints because it canonicalizes paths.
windows=0
case "$(uname -s)" in MINGW* | MSYS* | CYGWIN*) windows=1 ;; esac

# The native spelling of a POSIX path, backslashes doubled so it can sit inside a JSON string.
native_json() { # <path>
  local p=$1
  if [ "$windows" = 1 ]; then
    p=$(cygpath -w -l "$1")
    p=${p//\\/\\\\}
  fi
  printf '%s' "$p"
}

# Runs a command with the repository's pinned toolchains on PATH, in the caller's directory.
with_tools() {
  if [ -x "$mise" ]; then
    local here_dir=$PWD
    (cd "$repo" && "$mise" exec -- sh -c 'cd "$0" && exec "$@"' "$here_dir" "$@")
  else
    "$@"
  fi
}

if [ -z "${HOOKAH_BIN:-}" ]; then
  echo "building hookah (set HOOKAH_BIN to skip)" >&2
  with_tools cargo build --release --quiet --manifest-path "$repo/Cargo.toml" || exit 1
  HOOKAH_BIN=$repo/target/release/hookah
  [ "$windows" = 0 ] || HOOKAH_BIN=$HOOKAH_BIN.exe
fi
case "$HOOKAH_BIN" in /* | [A-Za-z]:*) ;; *) HOOKAH_BIN=$PWD/$HOOKAH_BIN ;; esac

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
work_real=$(cd "$work" && pwd -P)

# Session-start events compute their baseline before the hook returns, so the next event sees it.
export HOOKAH_BASELINE_FOREGROUND=1
export GIT_CONFIG_GLOBAL=/dev/null
export GIT_CONFIG_SYSTEM=/dev/null
# Python reuses a cached .pyc when the source has the same mtime (to the second) and size, which
# a scenario reset produces. Scenario runs never write bytecode, and resets delete it.
export PYTHONDONTWRITEBYTECODE=1
# No line-ending conversion: the patches and expected outputs are LF, also on Windows.
git_() { git -c user.name=samples -c user.email=samples@example.invalid -c core.autocrlf=false "$@"; }

pass=0
fail=0
skip=0
failed=""

# Replaces the project copy path (with and without the macOS /private prefix, and in every
# spelling Windows has) by <root>, and drops carriage returns.
normalize() { # <copy>
  local text real
  real=$work_real/${1##*/}
  text=$(cat)
  text=${text//$'\r'/}
  if [ "$windows" = 1 ]; then
    local win mixed escaped
    win=$(cygpath -w -l "$1")
    mixed=$(cygpath -m -l "$1")
    escaped=${win//\\/\\\\}
    text=${text//"$escaped"/<root>}
    text=${text//"$win"/<root>}
    text=${text//"$mixed"/<root>}
  fi
  text=${text//"$real"/<root>}
  text=${text//"$1"/<root>}
  printf '%s\n' "$text"
}

# Prints the normalized result of one hook call: exit code, stdout, stderr.
# <session> is the session id of the payload; empty means the payload carries none.
run_hook() { # <copy> <event-line> <out-file> <session>
  local copy=$1 event=$2 session=$4 payload code sid="" cwd file rel
  cwd=$(native_json "$copy")
  [ -z "$session" ] || sid=$(printf '"session_id":"%s",' "$session")
  case "$event" in
    start)
      payload=$(printf '{%s"hook_event_name":"SessionStart","source":"startup","cwd":"%s"}' "$sid" "$cwd")
      ;;
    "edit "*)
      rel=${event#edit }
      file=$cwd/$rel
      if [ "$windows" = 1 ]; then
        # A native path uses one separator all the way, as the harness sends it.
        file=$cwd\\\\${rel//\//\\\\}
      fi
      payload=$(printf '{%s"hook_event_name":"PostToolUse","tool_name":"Edit","cwd":"%s","tool_input":{"file_path":"%s"}}' \
        "$sid" "$cwd" "$file")
      ;;
    stop)
      payload=$(printf '{%s"hook_event_name":"Stop","stop_hook_active":false,"cwd":"%s"}' "$sid" "$cwd")
      ;;
    *)
      echo "bad event: $event" >&2
      return 1
      ;;
  esac
  printf '%s' "$payload" |
    with_tools sh -c 'cd "$1" && exec "$2" hook claude' sh "$copy" "$HOOKAH_BIN" \
      >"$work/stdout" 2>"$work/stderr"
  code=$?
  # Pretty-print JSON stdout when jq is available; jq output is deterministic.
  if command -v jq >/dev/null 2>&1 && jq -e . "$work/stdout" >/dev/null 2>&1; then
    jq . "$work/stdout" >"$work/stdout.pretty" && mv "$work/stdout.pretty" "$work/stdout"
  fi
  {
    echo "exit: $code"
    echo "--- stdout"
    cat "$work/stdout"
    echo "--- stderr"
    cat "$work/stderr"
  } | normalize "$copy" >"$3"
}

for sample in "$here"/*/*/; do
  sample=${sample%/}
  [ -d "$sample/project" ] && [ -d "$sample/scenarios" ] || continue
  config=${sample#"$here"/}

  scenarios=""
  for s in "$sample"/scenarios/*/; do
    s=${s%/}
    [ -f "$s/change.patch" ] || continue
    case "$config/${s##*/}" in *"$filter"*) scenarios="$scenarios $s" ;; esac
  done
  [ -n "$scenarios" ] || continue

  # Pristine copy, optional setup, one initial commit.
  copy=$work/$(echo "$config" | tr / -)
  mkdir -p "$copy"
  cp -R "$sample/project/." "$copy/"
  if [ -f "$sample/scenarios/setup.sh" ]; then
    echo "setup $config" >&2
    with_tools sh -c 'cd "$1" && sh "$2"' sh "$copy" "$sample/scenarios/setup.sh" >"$work/setup.log" 2>&1 || {
      cat "$work/setup.log" >&2
      echo "FAIL  $config (setup)"
      fail=$((fail + 1))
      failed="$failed $config(setup)"
      continue
    }
  fi
  (cd "$copy" && git init -q -b main && git add -A && git_ commit -q -m pristine) || exit 1
  pristine=$(cd "$copy" && git rev-parse HEAD)

  for s in $scenarios; do
    name=$config/${s##*/}

    if [ -f "$s/requires" ] && ! with_tools sh -c 'cd "$1" && sh "$2"' sh "$copy" "$s/requires" >/dev/null 2>&1; then
      echo "SKIP  $name (requires not met)"
      skip=$((skip + 1))
      continue
    fi

    # Reset to the pristine commit, then add the state the session starts from (`base.patch`,
    # committed so it is not part of the agent's change).
    (cd "$copy" && git reset -q --hard "$pristine" && git clean -fdq && find . -name __pycache__ -type d -prune -exec rm -rf {} + &&
      if [ -f "$s/base.patch" ]; then git apply "$s/base.patch" && git add -A && git_ commit -q -m base; fi) || {
      echo "FAIL  $name (base.patch does not apply)"
      fail=$((fail + 1))
      failed="$failed $name"
      continue
    }

    # Events run in order with one session id; only the last answer is recorded. `change.patch`
    # is applied just before the first event that is not `start`. A scenario without a `start`
    # event sends no session id, so no baseline is involved.
    session=""
    grep -qx 'start' "$s/event" && session=sample-${s##*/}
    actual=$work/actual.out
    applied=0
    ok=1
    while IFS= read -r event || [ -n "$event" ]; do
      [ -n "$event" ] || continue
      if [ "$event" != start ] && [ "$applied" = 0 ]; then
        applied=1
        (cd "$copy" && git apply "$s/change.patch") || {
          echo "FAIL  $name (patch does not apply)"
          ok=0
          break
        }
      fi
      if [ "$event" = clean ]; then
        # `clean` asserts that hookah has written nothing into the project: every changed or new
        # file must be one that `change.patch` itself touches.
        extra=$(cd "$copy" && git status --porcelain --untracked-files=all | sed 's/^...//' |
          grep -vxF -f <(git apply --numstat "$s/change.patch" | cut -f3) || true)
        if [ -n "$extra" ]; then
          echo "FAIL  $name (hookah wrote into the project)"
          echo "$extra"
          ok=0
          break
        fi
        continue
      fi
      run_hook "$copy" "$event" "$actual" "$session" || {
        echo "FAIL  $name (bad event)"
        ok=0
        break
      }
    done <"$s/event"
    if [ "$ok" = 0 ]; then
      fail=$((fail + 1))
      failed="$failed $name"
      continue
    fi

    if [ "$update" = 1 ]; then
      cp "$actual" "$s/expected.out"
      echo "WROTE $name"
    elif [ -f "$s/expected.out" ] && diff -u "$s/expected.out" "$actual" >"$work/diff"; then
      echo "PASS  $name"
      pass=$((pass + 1))
    else
      echo "FAIL  $name"
      if [ -f "$s/expected.out" ]; then cat "$work/diff"; else echo "(no expected.out)"; fi
      fail=$((fail + 1))
      failed="$failed $name"
    fi
  done
done

echo
echo "passed: $pass, failed: $fail, skipped: $skip"
[ -z "$failed" ] || echo "failed:$failed"
[ "$fail" -eq 0 ]
