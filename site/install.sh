#!/bin/sh
# hookah installer for Linux and macOS.
#
#   curl -fsSL https://izevg.github.io/hookah/install.sh | sh
#   curl -fsSL https://izevg.github.io/hookah/install.sh | sh -s -- --version 1.2.3 --dir ~/bin
#
# Options:
#   --version VERSION   install this release instead of the latest (same as HOOKAH_VERSION)
#   --dir DIR           install into DIR (same as HOOKAH_INSTALL_DIR; default: $HOME/.local/bin)
#   --dry-run           resolve the version and print what would happen; download and install nothing
#   -h, --help          show this help
#
# Environment:
#   HOOKAH_VERSION, HOOKAH_INSTALL_DIR   as above
#   HOOKAH_DOWNLOAD_BASE   where release assets live (default: GitHub release downloads); used by
#                          the installer tests against a local fake release
#   HOOKAH_API_URL         the releases API base, used to find the latest version (same purpose)
#
# It downloads the archive and SHA256SUMS, verifies the checksum, and installs one binary.
# No sudo, nothing outside the install directory.

set -eu

REPO="izevg/hookah"
DOWNLOAD_BASE="${HOOKAH_DOWNLOAD_BASE:-https://github.com/$REPO/releases/download}"
API_URL="${HOOKAH_API_URL:-https://api.github.com/repos/$REPO/releases}"

say() {
  printf 'hookah-install: %s\n' "$*"
}

err() {
  printf 'hookah-install: error: %s\n' "$*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
hookah installer for Linux and macOS.

Usage: install.sh [--version VERSION] [--dir DIR] [--dry-run]

  --version VERSION   install this release instead of the latest (or set HOOKAH_VERSION)
  --dir DIR           install into DIR (or set HOOKAH_INSTALL_DIR; default: $HOME/.local/bin)
  --dry-run           print what would happen; download and install nothing
  -h, --help          show this help

The archive is verified against the release's SHA256SUMS before anything is installed.
EOF
}

have() {
  command -v "$1" >/dev/null 2>&1
}

# fetch URL FILE: download URL into FILE.
fetch() {
  if have curl; then
    curl -fsSL --retry 3 -o "$2" "$1" || err "download failed: $1"
  elif have wget; then
    wget -q -O "$2" "$1" || err "download failed: $1"
  else
    err "need curl or wget to download files"
  fi
}

# fetch_stdout URL: print the response body; fail quietly (callers handle the failure).
fetch_stdout() {
  if have curl; then
    curl -fsSL --retry 3 "$1"
  elif have wget; then
    wget -q -O - "$1"
  else
    err "need curl or wget to download files"
  fi
}

# tag_from_json: read JSON on stdin, print the first tag_name value.
tag_from_json() {
  grep -o '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' | head -n 1 | sed 's/.*"\([^"]*\)"$/\1/'
}

sha256_of() {
  if have sha256sum; then
    sha256sum "$1" | awk '{print $1}'
  elif have shasum; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    err "need sha256sum or shasum to verify the download"
  fi
}

detect_target() {
  os="$(uname -s)"
  arch="$(uname -m)"
  case "$os" in
    Linux) os_part="unknown-linux-musl" ;;
    Darwin) os_part="apple-darwin" ;;
    *) err "unsupported operating system: $os (on Windows use install.ps1, see https://izevg.github.io/hookah/)" ;;
  esac
  # A shell running under Rosetta reports x86_64 on an Apple Silicon Mac: prefer the native binary.
  if [ "$os" = Darwin ] && [ "$arch" = x86_64 ] &&
    [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || true)" = 1 ]; then
    arch="arm64"
  fi
  case "$arch" in
    x86_64 | amd64) arch_part="x86_64" ;;
    aarch64 | arm64) arch_part="aarch64" ;;
    *) err "unsupported CPU architecture: $arch (supported: x86_64, aarch64)" ;;
  esac
  target="$arch_part-$os_part"
}

resolve_latest() {
  tag="$(fetch_stdout "$API_URL/latest" 2>/dev/null | tag_from_json || true)"
  if [ -z "$tag" ]; then
    # No stable release yet: fall back to the newest release of any kind.
    tag="$(fetch_stdout "$API_URL?per_page=1" 2>/dev/null | tag_from_json || true)"
    [ -n "$tag" ] && say "no stable release found, using the newest prerelease: $tag"
  fi
  [ -n "$tag" ] || err "could not find the latest release at $API_URL (set HOOKAH_VERSION or pass --version)"
  version="${tag#v}"
}

version="${HOOKAH_VERSION:-}"
dir="${HOOKAH_INSTALL_DIR:-$HOME/.local/bin}"
dry_run=0

while [ $# -gt 0 ]; do
  case "$1" in
    --version)
      [ $# -ge 2 ] || err "--version needs a value"
      version="$2"
      shift 2
      ;;
    --version=*)
      version="${1#--version=}"
      shift
      ;;
    --dir)
      [ $# -ge 2 ] || err "--dir needs a value"
      dir="$2"
      shift 2
      ;;
    --dir=*)
      dir="${1#--dir=}"
      shift
      ;;
    --dry-run)
      dry_run=1
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *) err "unknown option: $1 (try --help)" ;;
  esac
done

have uname || err "uname is required"
detect_target

if [ -n "$version" ]; then
  version="${version#v}"
else
  resolve_latest
fi
case "$version" in
  [0-9]*) ;;
  *) err "invalid version: $version" ;;
esac
case "$version" in
  *[!0-9A-Za-z.+-]*) err "invalid version: $version" ;;
esac

archive="hookah-$version-$target.tar.gz"
url="$DOWNLOAD_BASE/v$version/$archive"
sums_url="$DOWNLOAD_BASE/v$version/SHA256SUMS"

say "version: $version"
say "target:  $target"
say "archive: $url"
say "install: $dir/hookah"

if [ "$dry_run" = 1 ]; then
  say "dry run: nothing downloaded or installed"
  exit 0
fi

have tar || err "tar is required"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT HUP TERM

say "downloading $archive"
fetch "$url" "$tmp/$archive"
fetch "$sums_url" "$tmp/SHA256SUMS"

expected="$(awk -v f="$archive" '{ n = $2; sub(/^\*/, "", n); if (n == f) { print $1; exit } }' "$tmp/SHA256SUMS")"
[ -n "$expected" ] || err "SHA256SUMS has no entry for $archive"
actual="$(sha256_of "$tmp/$archive")"
expected="$(printf '%s' "$expected" | tr 'A-F' 'a-f')"
actual="$(printf '%s' "$actual" | tr 'A-F' 'a-f')"
if [ "$expected" != "$actual" ]; then
  err "checksum mismatch for $archive: expected $expected, got $actual (nothing installed)"
fi
say "checksum ok"

tar -xzf "$tmp/$archive" -C "$tmp" || err "could not extract $archive"
bin="$tmp/hookah-$version-$target/hookah"
[ -f "$bin" ] || err "the archive does not contain hookah-$version-$target/hookah"

mkdir -p "$dir" || err "cannot create $dir (pick another with --dir)"
cp "$bin" "$dir/.hookah.$$" || err "cannot write to $dir (pick another with --dir)"
chmod 755 "$dir/.hookah.$$"
mv -f "$dir/.hookah.$$" "$dir/hookah"
say "installed $("$dir/hookah" --version 2>/dev/null || echo "hookah $version") to $dir/hookah"

case ":$PATH:" in
  *":$dir:"*) ;;
  *)
    say "$dir is not on your PATH. Add it, for example:"
    say "  export PATH=\"$dir:\$PATH\""
    say "(put that line in ~/.profile, ~/.zshrc, or ~/.bashrc), or run $dir/hookah by full path."
    ;;
esac

say "next steps:"
say "  hookah setup --detect      # which agent harnesses are installed"
say "  hookah setup <harness>     # claude, codex, cursor, qoder, kimi, opencode, hermes, pi, or openclaw"
