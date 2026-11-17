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
fi
case "$HOOKAH_BIN" in /*) ;; *) HOOKAH_BIN=$PWD/$HOOKAH_BIN ;; esac

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
work_real=$(cd "$work" && pwd -P)

export GIT_CONFIG_GLOBAL=/dev/null
export GIT_CONFIG_SYSTEM=/dev/null
git_() { git -c user.name=samples -c user.email=samples@example.invalid "$@"; }

pass=0
fail=0
skip=0
failed=""

# Replaces the project copy path (with and without the macOS /private prefix) by <root>.
normalize() { # <copy>
  local text real
  real=$work_real/${1##*/}
  text=$(cat)
  text=${text//"$real"/<root>}
  text=${text//"$1"/<root>}
  printf '%s\n' "$text"
}

# Prints the normalized result of one hook call: exit code, stdout, stderr.
run_hook() { # <copy> <event-line> <out-file>
  local copy=$1 event=$2 payload code
  case "$event" in
    "edit "*)
      payload=$(printf '{"hook_event_name":"PostToolUse","tool_name":"Edit","cwd":"%s","tool_input":{"file_path":"%s/%s"}}' \
        "$copy" "$copy" "${event#edit }")
      ;;
    stop)
      payload=$(printf '{"hook_event_name":"Stop","stop_hook_active":false,"cwd":"%s"}' "$copy")
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

  for s in $scenarios; do
    name=$config/${s##*/}

    if [ -f "$s/requires" ] && ! with_tools sh -c 'cd "$1" && sh "$2"' sh "$copy" "$s/requires" >/dev/null 2>&1; then
      echo "SKIP  $name (requires not met)"
      skip=$((skip + 1))
      continue
    fi

    (cd "$copy" && git checkout -q -- . && git clean -fdq && git apply "$s/change.patch") || {
      echo "FAIL  $name (patch does not apply)"
      fail=$((fail + 1))
      failed="$failed $name"
      continue
    }

    actual=$work/actual.out
    run_hook "$copy" "$(head -n 1 "$s/event")" "$actual" || {
      echo "FAIL  $name (bad event)"
      fail=$((fail + 1))
      failed="$failed $name"
      continue
    }

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
