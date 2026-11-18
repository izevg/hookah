#!/usr/bin/env bash
# Tests site/install.sh (and site/install.ps1 when pwsh exists) against a fake release served
# from a local directory by `python3 -m http.server`. Nothing touches the network or the real PATH
# and home directory.
# Usage: .github/scripts/test-install.sh
set -eu

root="$(cd "$(dirname "$0")/../.." && pwd)"
sh_installer="$root/site/install.sh"
ps_installer="$root/site/install.ps1"
work="$(mktemp -d)"
server_pid=""
cleanup() {
  if [ -n "$server_pid" ]; then
    kill "$server_pid" 2> /dev/null || true
    wait "$server_pid" 2> /dev/null || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT

failures=0
pass() { printf 'ok   %s\n' "$1"; }
fail() {
  printf 'FAIL %s\n' "$1"
  failures=$((failures + 1))
}

# --- the fake release ---------------------------------------------------------------------------
www="$work/www"
mkdir -p "$www"
sha256() { if command -v sha256sum >/dev/null; then sha256sum "$@"; else shasum -a 256 "$@"; fi; }

make_release() { # make_release VERSION
  local v="$1" rel="$www/v$1" t
  mkdir -p "$rel"
  for t in x86_64-unknown-linux-musl aarch64-unknown-linux-musl x86_64-apple-darwin aarch64-apple-darwin; do
    mkdir -p "$work/stage/hookah-$v-$t"
    # The "binary" is a script that names its own target, so tests can tell the archives apart.
    printf '#!/bin/sh\necho "hookah %s (%s)"\n' "$v" "$t" > "$work/stage/hookah-$v-$t/hookah"
    chmod 755 "$work/stage/hookah-$v-$t/hookah"
    echo license > "$work/stage/hookah-$v-$t/LICENSE"
    echo readme > "$work/stage/hookah-$v-$t/README.md"
    tar -czf "$rel/hookah-$v-$t.tar.gz" -C "$work/stage" "hookah-$v-$t"
  done
  for t in x86_64-pc-windows-msvc aarch64-pc-windows-msvc; do
    mkdir -p "$work/stage/hookah-$v-$t"
    printf '%s' "$t" > "$work/stage/hookah-$v-$t/hookah.exe"
    python3 -I - "$work/stage" "hookah-$v-$t" "$rel/hookah-$v-$t.zip" << 'PY'
import sys, zipfile, pathlib
stage, name, out = sys.argv[1:]
with zipfile.ZipFile(out, "w") as z:
    for p in sorted(pathlib.Path(stage, name).rglob("*")):
        z.write(p, p.relative_to(stage).as_posix())
PY
  done
  (cd "$rel" && sha256 hookah-* > SHA256SUMS)
}

make_release 1.2.3
make_release 1.2.4
make_release 1.3.0-rc.1
# 1.2.4 gets a wrong checksum for the x86_64 linux and windows archives; 1.2.3 has its entry removed for the arm one.
for broken in unknown-linux-musl.tar.gz pc-windows-msvc.zip; do
  sed -i.bak "s/^[0-9a-f]\\{64\\}\\(  hookah-1.2.4-x86_64-$broken\\)/0000000000000000000000000000000000000000000000000000000000000000\\1/" "$www/v1.2.4/SHA256SUMS"
done
rm "$www/v1.2.4/SHA256SUMS.bak"
grep -v 'aarch64-unknown-linux-musl' "$www/v1.2.3/SHA256SUMS" > "$www/v1.2.3/SHA256SUMS.new"
mv "$www/v1.2.3/SHA256SUMS.new" "$www/v1.2.3/SHA256SUMS"
# The releases API: `latest` points at 1.2.3; the second API has only a prerelease.
mkdir -p "$www/api" "$www/api-pre"
printf '{\n  "tag_name": "v1.2.3",\n  "name": "v1.2.3"\n}\n' > "$www/api/latest"
printf '[{\n  "tag_name": "v1.3.0-rc.1"\n}]\n' > "$www/api-pre/index.html"

python3 -u -m http.server 0 --bind 127.0.0.1 --directory "$www" > "$work/server.log" 2>&1 &
server_pid=$!
port=""
for _ in $(seq 1 50); do
  port="$(sed -n 's/.*port \([0-9][0-9]*\).*/\1/p' "$work/server.log" | head -n 1)"
  [ -n "$port" ] && break
  sleep 0.1
done
[ -n "$port" ] || {
  echo "http.server did not start" >&2
  cat "$work/server.log" >&2
  exit 1
}
base="http://127.0.0.1:$port"

# --- shims --------------------------------------------------------------------------------------
shim="$work/shim"
mkdir -p "$shim"
cat > "$shim/uname" << 'EOF'
#!/bin/sh
case "$1" in
  -s) echo "$FAKE_OS" ;;
  -m) echo "$FAKE_ARCH" ;;
  *) echo "$FAKE_OS" ;;
esac
EOF
chmod +x "$shim/uname"

# run_sh OS ARCH ARGS...: run the installer as that platform. Output goes to $work/out.
run_sh() {
  local os="$1" arch="$2"
  shift 2
  FAKE_OS="$os" FAKE_ARCH="$arch" PATH="$shim:$PATH" \
    HOOKAH_DOWNLOAD_BASE="$base" HOOKAH_API_URL="$base/api" HOME="$work/home" \
    sh "$sh_installer" "$@" > "$work/out" 2>&1
}

check() { # check NAME WANT_STATUS PATTERN -- the last run_sh status is in $status
  local name="$1" want="$2" pattern="${3:-}"
  if [ "$status" -ne "$want" ]; then
    fail "$name (exit $status, wanted $want)"
    sed 's/^/     /' "$work/out"
  elif [ -n "$pattern" ] && ! grep -q -- "$pattern" "$work/out"; then
    fail "$name (output lacks: $pattern)"
    sed 's/^/     /' "$work/out"
  else
    pass "$name"
  fi
}

assert() { # assert NAME CMD...: pass when CMD succeeds
  local name="$1"
  shift
  if "$@"; then pass "$name"; else fail "$name"; fi
}
assert_eq() { # assert_eq NAME ACTUAL EXPECTED
  if [ "$2" = "$3" ]; then pass "$1"; else fail "$1 (got '$2', wanted '$3')"; fi
}

go() { # go OS ARCH ARGS...: run and record status
  status=0
  run_sh "$@" || status=$?
}

# --- OS / arch mapping --------------------------------------------------------------------------
go Linux x86_64 --dry-run --version 1.2.3
check "maps Linux x86_64" 0 "target:  x86_64-unknown-linux-musl"
go Linux aarch64 --dry-run --version 1.2.3
check "maps Linux aarch64" 0 "target:  aarch64-unknown-linux-musl"
go Linux arm64 --dry-run --version 1.2.3
check "maps Linux arm64" 0 "target:  aarch64-unknown-linux-musl"
go Darwin x86_64 --dry-run --version 1.2.3
check "maps Darwin x86_64" 0 "target:  x86_64-apple-darwin"
go Darwin arm64 --dry-run --version 1.2.3
check "maps Darwin arm64" 0 "target:  aarch64-apple-darwin"
go Linux riscv64 --dry-run --version 1.2.3
check "rejects an unsupported arch" 1 "unsupported CPU architecture"
go FreeBSD x86_64 --dry-run --version 1.2.3
check "rejects an unsupported OS" 1 "unsupported operating system"
go MINGW64_NT x86_64 --dry-run --version 1.2.3
check "points Windows shells at install.ps1" 1 "install.ps1"

# --- install paths ------------------------------------------------------------------------------
dest="$work/dest1"
go Linux x86_64 --dir "$dest" --dry-run --version 1.2.3
check "dry run prints the plan" 0 "dry run"
assert "dry run installs nothing" test ! -e "$dest"

go Linux x86_64 --dir "$dest" --version 1.2.3
check "installs an explicit version" 0 "checksum ok"
if [ "$("$dest/hookah" --version)" = "hookah 1.2.3 (x86_64-unknown-linux-musl)" ]; then pass "installed binary is the right one"; else fail "installed binary mismatch"; fi
if grep -q 'is not on your PATH' "$work/out"; then pass "prints PATH advice when the dir is off PATH"; else fail "no PATH advice"; fi
assert "prints next steps" grep -q 'hookah setup --detect' "$work/out"

dest="$work/dest2"
status=0
FAKE_OS=Darwin FAKE_ARCH=arm64 PATH="$dest:$shim:$PATH" HOOKAH_DOWNLOAD_BASE="$base" HOOKAH_API_URL="$base/api" \
  HOOKAH_INSTALL_DIR="$dest" HOOKAH_VERSION=v1.2.4 HOME="$work/home" sh "$sh_installer" > "$work/out" 2>&1 || status=$?
check "HOOKAH_VERSION and HOOKAH_INSTALL_DIR work, v prefix accepted" 0 "checksum ok"
if grep -q 'is not on your PATH' "$work/out"; then fail "PATH advice shown though dir is on PATH"; else pass "no PATH advice when the dir is on PATH"; fi
assert_eq "darwin arm64 archive installed" "$("$dest/hookah" --version)" "hookah 1.2.4 (aarch64-apple-darwin)"

dest="$work/dest3"
go Linux x86_64 --dir "$dest"
check "resolves the latest version from the API" 0 "version: 1.2.3"

dest="$work/dest-pre"
status=0
FAKE_OS=Linux FAKE_ARCH=x86_64 PATH="$shim:$PATH" HOOKAH_DOWNLOAD_BASE="$base" HOOKAH_API_URL="$base/api-pre" HOME="$work/home" \
  sh "$sh_installer" --dir "$dest" > "$work/out" 2>&1 || status=$?
check "falls back to a prerelease when there is no stable one" 0 "version: 1.3.0-rc.1"

go Linux x86_64 --dir "$work/dest-missing" --version 9.9.9
check "fails clearly when the release does not exist" 1 "download failed"

# --- checksum failure paths ---------------------------------------------------------------------
dest="$work/dest-bad"
go Linux x86_64 --dir "$dest" --version 1.2.4
check "aborts on a checksum mismatch" 1 "checksum mismatch"
assert "nothing installed after a mismatch" test ! -e "$dest/hookah"
go Linux aarch64 --dir "$dest" --version 1.2.3
check "aborts when SHA256SUMS lacks the archive" 1 "no entry for"

# --- wget instead of curl (only where wget exists) ----------------------------------------------
if command -v wget > /dev/null; then
  only="$work/only-wget"
  mkdir -p "$only"
  for tool in sh cat awk sed grep head tr mktemp tar gzip rm cp mv chmod mkdir sysctl dirname wget sha256sum shasum; do
    p="$(command -v "$tool" 2> /dev/null || true)"
    [ -n "$p" ] && ln -sf "$p" "$only/$tool"
  done
  cp "$shim/uname" "$only/uname"
  dest="$work/dest-wget"
  status=0
  FAKE_OS=Linux FAKE_ARCH=x86_64 PATH="$only" HOOKAH_DOWNLOAD_BASE="$base" HOOKAH_API_URL="$base/api" HOME="$work/home" \
    "$only/sh" "$sh_installer" --dir "$dest" > "$work/out" 2>&1 || status=$?
  check "works with wget only" 0 "checksum ok"
else
  echo "skip wget-only test: wget not installed"
fi

# --- install.ps1 --------------------------------------------------------------------------------
if command -v pwsh > /dev/null; then
  run_ps() { # run_ps ARCH VERSION DIR
    status=0
    PROCESSOR_ARCHITECTURE="$1" PROCESSOR_ARCHITEW6432="" HOOKAH_VERSION="$2" HOOKAH_INSTALL_DIR="$3" HOOKAH_NO_MODIFY_PATH=1 \
      HOOKAH_DOWNLOAD_BASE="$base" HOOKAH_API_URL="$base/api" \
      pwsh -NoProfile -NonInteractive -File "$ps_installer" > "$work/out" 2>&1 || status=$?
  }
  run_ps AMD64 1.2.3 "$work/ps1"
  check "install.ps1 installs (AMD64)" 0 "checksum ok"
  assert_eq "install.ps1 maps AMD64" "$(cat "$work/ps1/hookah.exe" 2> /dev/null)" "x86_64-pc-windows-msvc"
  run_ps ARM64 v1.2.3 "$work/ps2"
  assert_eq "install.ps1 maps ARM64" "$(cat "$work/ps2/hookah.exe" 2> /dev/null)" "aarch64-pc-windows-msvc"
  run_ps AMD64 1.2.4 "$work/ps3"
  check "install.ps1 aborts on a bad checksum" 1 "checksum mismatch"
  assert "install.ps1 installs nothing after a mismatch" test ! -e "$work/ps3/hookah.exe"
  run_ps x86 1.2.3 "$work/ps4"
  check "install.ps1 rejects an unsupported arch" 1 "unsupported CPU architecture"
  status=0
  PROCESSOR_ARCHITECTURE=AMD64 PROCESSOR_ARCHITEW6432="" HOOKAH_VERSION="" HOOKAH_INSTALL_DIR="$work/ps5" HOOKAH_NO_MODIFY_PATH=1 \
    HOOKAH_DOWNLOAD_BASE="$base" HOOKAH_API_URL="$base/api" pwsh -NoProfile -NonInteractive -File "$ps_installer" > "$work/out" 2>&1 || status=$?
  check "install.ps1 resolves the latest version" 0 "version: 1.2.3"
else
  echo "skip install.ps1 tests: pwsh not installed"
fi

if [ "$failures" -gt 0 ]; then
  echo "$failures installer test(s) failed" >&2
  exit 1
fi
echo "installer tests: ok"
