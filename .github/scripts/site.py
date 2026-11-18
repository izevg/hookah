#!/usr/bin/env python3
"""Assemble, finish, and check the documentation site. Run through build-site.sh.

  site.py assemble   copy docs/, the README, and site/ into target/site/ (the mdBook root)
  site.py finish     add raw markdown, installers, llms.txt, and llms-full.txt to the built book
  site.py check      verify every internal link and anchor in the built site

docs/ stays the single source of truth. The book lives one level above it: docs/X.md becomes
/X.html, so links are rewritten while copying (links to files outside docs/ become GitHub links).
"""

import html.parser
import os
import re
import shlex
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
OUT = ROOT / "target" / "site"
SRC = OUT / "src"
BOOK = OUT / "book"
SITE_URL = os.environ.get("HOOKAH_SITE_URL", "https://izevg.github.io/hookah").rstrip("/")
REPO_URL = "https://github.com/izevg/hookah"

LINK = re.compile(r"(\]\()([^)\s]+)((?:\s+\"[^\"]*\")?\))")
SRC_ATTR = re.compile(r"""(\b(?:src|href)=")([^"]+)(")""")
EXTERNAL = re.compile(r"^(?:[a-z][a-z0-9+.-]*:|//|#)", re.I)

DESCRIPTIONS = {
    "README.md": "what hookah is, install commands, and a quick start",
    "setup/agents.md": "the exact procedure an agent follows to install and configure hookah",
    "setup/uninstall.md": "hookah uninstall, the manual removal per harness, and how to verify that nothing is left",
    "configuration.md": ".hookah.toml and the cache",
    "ecosystems.md": "detection, commands, output parsing, and test selection for Rust, Node, Python, and Java",
    "architecture.md": "layers, request flow, baseline, and failure handling",
    "mutation.md": "mutation testing of hookah itself",
    "changelog.md": "every release, generated from Conventional Commits",
}

# The changelog page: one section per release, newest first, plus "Unreleased" for the commits
# after the last `v*` tag (all of them before the first release). Same commit rules as the release
# notes, so the groups are the ones in .github/cliff.toml.
CHANGELOG_BODY = """\
{% if version %}## {{ version | trim_start_matches(pat="v") }} ({{ timestamp | date(format="%Y-%m-%d") }})
{% else %}## Unreleased
{% endif %}\
{% for group, commits in commits | group_by(attribute="group") %}
### {{ group | striptags | trim | upper_first }}

{% for commit in commits %}\
- {% if commit.scope %}**{{ commit.scope }}**: {% endif %}\
{% if commit.breaking %}**BREAKING** {% endif %}\
{{ commit.message | upper_first }} \
([`{{ commit.id | truncate(length=7, end="") }}`](https://github.com/izevg/hookah/commit/{{ commit.id }}))
{% endfor %}\
{% endfor %}
"""


def rewrite_links(text, mapper):
    """Apply mapper(target) to every relative markdown link and HTML src/href, outside code fences."""
    out, fenced = [], False
    for line in text.split("\n"):
        if line.lstrip().startswith(("```", "~~~")):
            fenced = not fenced
        elif not fenced:
            line = LINK.sub(lambda m: m.group(1) + map_target(m.group(2), mapper) + m.group(3), line)
            line = SRC_ATTR.sub(lambda m: m.group(1) + map_target(m.group(2), mapper) + m.group(3), line)
        out.append(line)
    return "\n".join(out)


def map_target(target, mapper):
    if EXTERNAL.match(target):
        return target
    path, sep, fragment = target.partition("#")
    return mapper(path) + sep + fragment


def to_book_link(repo_file):
    """Mapper factory: links in repo_file (a path relative to the repo root) -> links in src/."""
    here = Path(repo_file).parent
    new_here = here.relative_to("docs") if here.parts[:1] == ("docs",) else Path(".")

    def mapper(path):
        if not path:
            return path
        resolved = Path(os.path.normpath(here / path))
        target = ROOT / resolved
        if resolved.parts[:1] == ("docs",):
            return Path(os.path.relpath(resolved.relative_to("docs"), new_here)).as_posix()
        if target.is_dir():
            return f"{REPO_URL}/tree/main/{resolved.as_posix()}"
        if target.exists():
            return f"{REPO_URL}/blob/main/{resolved.as_posix()}"
        return path  # left alone: the link check reports it

    return mapper


def to_absolute_link(src_file):
    """Mapper factory for llms-full.txt: links in src/<src_file> -> absolute site URLs."""
    here = Path(src_file).parent

    def mapper(path):
        if not path:
            return path
        resolved = Path(os.path.normpath(here / path)).as_posix()
        if resolved == "README.md":
            resolved = "index.md"
        return f"{SITE_URL}/{resolved}"

    return mapper


def changelog():
    """The changelog page, generated with git-cliff (on PATH, or the command in $CLIFF)."""
    command = [
        *shlex.split(os.environ.get("CLIFF", "git-cliff")),
        "--config", str(ROOT / ".github" / "cliff.toml"),
        "--body", CHANGELOG_BODY, "--strip", "all",
    ]
    try:
        done = subprocess.run(command, cwd=ROOT, capture_output=True, text=True, check=True)
    except (OSError, subprocess.CalledProcessError) as error:
        detail = getattr(error, "stderr", "") or error
        sys.exit(f"site: git-cliff failed (install it with mise; CI needs fetch-depth 0): {detail}")
    body = re.sub(r"\n{3,}", "\n\n", done.stdout.replace("\n## ", "\n\n## ")).strip()
    return "# Changelog\n\nGenerated from Conventional Commits at build time.\n\n" + (body or "No releasable changes yet.") + "\n"


def assemble():
    if OUT.exists():
        shutil.rmtree(OUT)
    SRC.mkdir(parents=True)
    for source in sorted((ROOT / "docs").rglob("*")):
        if not source.is_file():
            continue
        rel = source.relative_to(ROOT / "docs")
        dest = SRC / rel
        dest.parent.mkdir(parents=True, exist_ok=True)
        if source.suffix == ".md":
            text = source.read_text(encoding="utf-8")
            dest.write_text(rewrite_links(text, to_book_link(f"docs/{rel.as_posix()}")), encoding="utf-8")
        else:
            shutil.copy2(source, dest)
    readme = (ROOT / "README.md").read_text(encoding="utf-8")
    (SRC / "README.md").write_text(rewrite_links(readme, to_book_link("README.md")), encoding="utf-8")
    (SRC / "changelog.md").write_text(changelog(), encoding="utf-8")
    shutil.copy2(ROOT / "site" / "SUMMARY.md", SRC / "SUMMARY.md")
    shutil.copy2(ROOT / "site" / "book.toml", OUT / "book.toml")
    (OUT / "theme").mkdir()
    shutil.copy2(ROOT / "docs" / "assets" / "logo.svg", OUT / "theme" / "favicon.svg")


def summary_entries():
    """(section, title, path) for every chapter in SUMMARY.md, in order."""
    section, entries = "Overview", []
    for line in (SRC / "SUMMARY.md").read_text(encoding="utf-8").splitlines():
        heading = re.match(r"^#\s+(.+)$", line)
        item = re.match(r"^\s*[-*]\s+\[([^\]]+)\]\(([^)]+)\)", line)
        if heading:
            section = heading.group(1).strip()
        elif item:
            entries.append((section, item.group(1), item.group(2)))
    return entries


def raw_url(path):
    return f"{SITE_URL}/{'index.md' if path == 'README.md' else path}"


def finish():
    for source in SRC.rglob("*.md"):
        rel = source.relative_to(SRC)
        if rel.name == "SUMMARY.md":
            continue
        dest = BOOK / ("index.md" if rel.as_posix() == "README.md" else rel)
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, dest)
    for name in ("install.sh", "install.ps1"):
        shutil.copy2(ROOT / "site" / name, BOOK / name)
    (BOOK / ".nojekyll").write_text("", encoding="utf-8")

    entries = summary_entries()
    lines = [
        "# hookah",
        "",
        "> hookah is a single-binary CLI that AI coding agents call from their hooks after every file edit. "
        "It runs the project's own format, lint, typecheck, test, and mutation tools on what changed and "
        "feeds only the new failures back to the agent. It supports Claude Code, Codex CLI, Cursor, Qoder, Kimi "
        "Code, opencode, Hermes Agent, Pi, and OpenClaw, on Rust, Node (TypeScript/JavaScript), Python, and Java projects.",
        "",
        f"To install and configure hookah, follow [Let your agent install it]({SITE_URL}/setup/agents.md) "
        "step by step. In short: install the binary "
        f"(`curl -fsSL {SITE_URL}/install.sh | sh` on Linux and macOS, "
        f"`irm {SITE_URL}/install.ps1 | iex` on Windows), then run `hookah setup --detect` and "
        "`hookah setup <harness>`. Every page below is plain markdown.",
        "",
        "If the user says \"pack me a hookah\", that means: install and configure hookah (the procedure above). "
        f"\"Put out my hookah\" means: uninstall it, following [Uninstall]({SITE_URL}/setup/uninstall.md) "
        "(`hookah uninstall --all`, then the installer's `--uninstall`).",
    ]
    current = None
    for section, title, path in entries:
        if section != current:
            lines += ["", f"## {section}", ""]
            current = section
        note = DESCRIPTIONS.get(path)
        lines.append(f"- [{title}]({raw_url(path)})" + (f": {note}" if note else ""))
    lines += [
        "",
        "## Optional",
        "",
        f"- [All documentation in one file]({SITE_URL}/llms-full.txt)",
        f"- [Source repository]({REPO_URL})",
        f"- [Releases and checksums]({REPO_URL}/releases)",
        "",
    ]
    (BOOK / "llms.txt").write_text("\n".join(lines), encoding="utf-8")

    parts = ["# hookah documentation (full text)", "", f"Source: {SITE_URL}/. Pages follow in reading order."]
    for _section, _title, path in entries:
        text = (SRC / path).read_text(encoding="utf-8")
        text = rewrite_links(text, to_absolute_link(path))
        parts += ["", "---", "", f"<!-- page: {raw_url(path)} -->", "", text.strip()]
    (BOOK / "llms-full.txt").write_text("\n".join(parts) + "\n", encoding="utf-8")


class Page(html.parser.HTMLParser):
    def __init__(self):
        super().__init__()
        self.links, self.ids = [], set()

    def handle_starttag(self, tag, attrs):
        attrs = dict(attrs)
        for key in ("id", "name"):
            if attrs.get(key):
                self.ids.add(attrs[key])
        for key in ("href", "src"):
            if attrs.get(key) and tag in ("a", "img", "link", "script", "source"):
                self.links.append(attrs[key])


def parse(path):
    page = Page()
    page.feed(path.read_text(encoding="utf-8"))
    return page


def check():
    problems, pages = [], {}

    def page_of(path):
        if path not in pages:
            pages[path] = parse(path)
        return pages[path]

    def resolve(base, target):
        if target.startswith(SITE_URL):
            target = target[len(SITE_URL):] or "/"
        elif EXTERNAL.match(target) and not target.startswith("#"):
            return None
        path, _, fragment = target.partition("#")
        if not path:
            return base, fragment
        found = (BOOK / path.lstrip("/")) if path.startswith("/") else (base.parent / path)
        found = Path(os.path.normpath(found))
        if found.is_dir():
            found = found / "index.html"
        return found, fragment

    for file in sorted(BOOK.rglob("*.html")):
        if file.name in ("404.html", "print.html"):
            continue
        for link in page_of(file).links:
            if link.startswith(("/", "http")) and not link.startswith(SITE_URL):
                if not link.startswith("//"):
                    if link.startswith("/"):
                        # root-relative links only appear in the 404 page; anything else is a bug
                        problems.append(f"{file.relative_to(BOOK)}: root-relative link {link}")
                    continue
            hit = resolve(file, link)
            if hit is None:
                continue
            found, fragment = hit
            if not found.is_file():
                problems.append(f"{file.relative_to(BOOK)}: broken link {link}")
            elif fragment and found.suffix == ".html" and fragment not in page_of(found).ids:
                problems.append(f"{file.relative_to(BOOK)}: missing anchor {link}")
    for file in sorted(BOOK.rglob("*.md")):
        text = file.read_text(encoding="utf-8")
        for match in LINK.finditer(text):
            target = match.group(2)
            if EXTERNAL.match(target) and not target.startswith(SITE_URL):
                continue
            hit = resolve(file, target)
            if hit and not hit[0].exists():
                problems.append(f"{file.relative_to(BOOK)}: broken link {target}")
    for name in ("llms.txt", "llms-full.txt", "install.sh", "install.ps1", "setup/agents.md", "index.md"):
        if not (BOOK / name).is_file():
            problems.append(f"missing published file {name}")
    if problems:
        print("\n".join(sorted(set(problems))), file=sys.stderr)
        sys.exit(1)
    print(f"site check: ok ({len(pages)} pages with links checked)")


if __name__ == "__main__":
    commands = {"assemble": assemble, "finish": finish, "check": check}
    if len(sys.argv) != 2 or sys.argv[1] not in commands:
        sys.exit(__doc__)
    commands[sys.argv[1]]()
