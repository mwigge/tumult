#!/usr/bin/env python3
"""Check documentation links and repository-derived inventory claims."""

from __future__ import annotations

import re
import subprocess
import sys
from pathlib import Path
from urllib.parse import unquote

ROOT = Path(__file__).resolve().parents[1]
DOC_FILES = (
    [
        ROOT / name
        for name in ("README.md", "QUICKSTART.md", "SECURITY.md", "CHANGELOG.md")
    ]
    + sorted((ROOT / "docs").rglob("*.md"))
    + [
        ROOT / "chaos-lab/README.md",
    ]
    + sorted((ROOT / "chaos-lab/docs").glob("*.md"))
)


# Paths the docs name that git does not hold: a build or the user makes them.
GENERATED_PATHS = {
    "web/build": "the build of the web UI, made by npm --prefix web run build",
    "chaos-lab/.env": "the file of local overrides that the user writes",
}
# Pages that record the past: they name paths that were true at their date.
DATED = ("docs/releases/", "docs/blog/")
HEADING = re.compile(r"^#{1,6}\s+(.+?)(?:\s+#+)?\s*$")
HTML_ANCHOR = re.compile(r"""\b(?:id|name)=["']([^"']+)["']""")
ANCHOR_LINK = re.compile(r"(?<!!)\[[^]]*\]\(([^)\s#]*)#([^)\s]+)\)")
CODE_SPAN = re.compile(r"`[^`]*`")
BACKTICKED = re.compile(r"`([^`\s]+)`")
LINE_SUFFIX = re.compile(r":\d+(-\d+)?(,\d+)*$")


def fail(errors: list[str], path: Path, message: str) -> None:
    errors.append(f"{path.relative_to(ROOT)}: {message}")


def repository_files(root: Path) -> list[str]:
    """Tracked files that are still there, and new files git does not ignore."""
    out = subprocess.run(
        ["git", "-C", str(root), "ls-files", "-z", "--cached", "--others"]
        + ["--exclude-standard"],
        capture_output=True,
        check=True,
    ).stdout.decode("utf-8")
    names = sorted({name for name in out.split("\0") if name})
    return [name for name in names if (root / name).is_file()]


def prose_lines(text: str) -> list[tuple[int, str]]:
    """The numbered lines of a page outside its fenced code blocks."""
    lines, fence = [], False
    for number, line in enumerate(text.splitlines(), 1):
        if line.lstrip().startswith(("```", "~~~")):
            fence = not fence
            continue
        if not fence:
            lines.append((number, line))
    return lines


def slug(heading: str) -> str:
    """The anchor of a heading, as GitHub and the site make it."""
    text = re.sub(r"<[^>]+>", "", heading)
    text = re.sub(r"\[([^]]*)\]\([^)]*\)", r"\1", text)
    text = re.sub(r"[^\w\- ]", "", text.replace("`", "").strip().lower())
    return text.replace(" ", "-")


def anchors(text: str) -> set[str]:
    """The headings of a page as anchors, a repeated one with `-1`, `-2`."""
    found: set[str] = set()
    seen: dict[str, int] = {}
    for _, line in prose_lines(text):
        found.update(HTML_ANCHOR.findall(line))
        heading = HEADING.match(line)
        if not heading:
            continue
        name = slug(heading[1])
        count = seen.get(name, 0)
        seen[name] = count + 1
        found.add(f"{name}-{count}" if count else name)
    return found


def check_anchors(errors: list[str], root: Path = ROOT) -> None:
    """Every link to `#anchor` of a markdown page lands on one of its
    headings: the page itself, or the `.md` page the link names."""
    pages = [name for name in repository_files(root) if name.endswith(".md")]
    for page in pages:
        path = root / page
        text = path.read_text(encoding="utf-8")
        for _, line in prose_lines(text):
            for target, anchor in ANCHOR_LINK.findall(CODE_SPAN.sub("", line)):
                if target and not target.endswith(".md"):
                    continue
                linked = (path.parent / unquote(target)).resolve() if target else path
                if not linked.is_file():
                    continue
                if anchor not in anchors(linked.read_text(encoding="utf-8")):
                    errors.append(
                        f"{page}: link to a missing heading: {target}#{anchor}"
                    )


def repository_path(token: str, top: set[str]) -> str | None:
    """The repository path a backticked token names, or None when it is not
    one: no `/`, a glob or a placeholder, or a first part that is not a
    top-level entry of the repository. A line suffix (`:12`) is dropped."""
    path = LINE_SUFFIX.sub("", token).rstrip("/.,")
    if "/" not in path or any(c in path for c in "*{}<>$?|"):
        return None
    return path if path.split("/", 1)[0] in top else None


def check_repo_paths(
    errors: list[str], root: Path = ROOT, generated: dict[str, str] = GENERATED_PATHS
) -> None:
    """Every backticked repository path of a markdown page exists, except in
    the dated records. A page of a sub-project (`chaos-lab/`) may also name a
    path from its own folder."""
    files = repository_files(root)
    present = set(files)
    for name in files:
        parts = name.split("/")
        present.update("/".join(parts[:end]) for end in range(1, len(parts)))
    top = {name.split("/", 1)[0] for name in files}
    named = set()
    for page in files:
        dated = page.endswith("CHANGELOG.md") or page.startswith(DATED)
        if not page.endswith(".md") or dated:
            continue
        project = page.split("/", 1)[0] + "/" if "/" in page else ""
        text = (root / page).read_text(encoding="utf-8")
        for number, line in enumerate(text.splitlines(), 1):
            for token in BACKTICKED.findall(line):
                path = repository_path(token, top)
                if path is None or path in present or project + path in present:
                    continue
                if path in generated:
                    named.add(path)
                    continue
                errors.append(f"{page}:{number}: no such repository path: {token}")
    for path in sorted(set(generated) - named):
        errors.append(
            f"scripts/check-docs.py: GENERATED_PATHS lists {path}, no page names it"
        )


def check_links(errors: list[str]) -> None:
    link_re = re.compile(r"(?<!!)\[[^]]*\]\(([^)]+)\)")
    for path in DOC_FILES:
        text = path.read_text(encoding="utf-8")
        for raw in link_re.findall(text):
            target = raw.strip().split(maxsplit=1)[0].strip("<>")
            if (
                not target
                or "{{" in target
                or "{%" in target
                or target.startswith(
                    ("#", "http://", "https://", "mailto:", "tumult://")
                )
            ):
                continue
            local = unquote(target.split("#", 1)[0])
            resolved = (path.parent / local).resolve()
            if ROOT not in resolved.parents and resolved != ROOT:
                fail(errors, path, f"link escapes the repository: {target}")
            elif not resolved.exists():
                fail(errors, path, f"broken local link: {target}")


def check_blog(errors: list[str]) -> None:
    posts = sorted((ROOT / "docs/blog").glob("[0-9][0-9]-*.md"))
    if len(posts) != 27:
        errors.append(f"docs/blog: expected 27 numbered posts, found {len(posts)}")
    index = (ROOT / "docs/blog/index.md").read_text(encoding="utf-8")
    for post in posts:
        text = post.read_text(encoding="utf-8")
        if not re.search(r"^updated: \d{4}-\d{2}-\d{2}$", text, re.M):
            fail(errors, post, "missing an `updated:` editorial review date")
        if f"({post.name})" not in index:
            fail(errors, ROOT / "docs/blog/index.md", f"missing {post.name}")

    discouraged = {
        "here's the thing": "replace canned rhetorical phrasing",
        "here’s the thing": "replace canned rhetorical phrasing",
        "here's the part": "replace canned rhetorical phrasing",
        "here’s the part": "replace canned rhetorical phrasing",
        "that's the whole point": "replace canned rhetorical phrasing",
        "that’s the whole point": "replace canned rhetorical phrasing",
        "nobody else ships": "remove unsupported competitor claim",
    }
    for post in posts:
        lowered = post.read_text(encoding="utf-8").lower()
        for phrase, message in discouraged.items():
            if phrase in lowered:
                fail(errors, post, f"{message}: {phrase!r}")


def check_mcp_inventory(errors: list[str]) -> None:
    schema_dir = ROOT / "tumult-mcp/src/handler/schema"
    source = "\n".join(
        path.read_text(encoding="utf-8") for path in schema_dir.glob("*.rs")
    )
    tools = set(re.findall(r'name\s*=\s*"(tumult_[a-z0-9_]+)"', source))
    if len(tools) != 40:
        errors.append(f"MCP schemas: expected 40 named tools, found {len(tools)}")

    output = (ROOT / "tumult-mcp/src/handler/output_schema.rs").read_text(
        encoding="utf-8"
    )
    match = re.search(r"STRUCTURED_TOOLS:\s*&\[&str\]\s*=\s*&\[(.*?)\];", output, re.S)
    structured = (
        set(re.findall(r'"(tumult_[a-z0-9_]+)"', match.group(1))) if match else set()
    )
    if len(structured) != 30:
        errors.append(
            f"structured MCP schemas: expected 30 tools, found {len(structured)}"
        )

    readme = (ROOT / "README.md").read_text(encoding="utf-8")
    documented = set(re.findall(r"`(tumult_[a-z0-9_]+)`", readme))
    missing = sorted(tools - documented)
    extra = sorted(documented - tools)
    if missing:
        errors.append(f"README MCP inventory missing: {', '.join(missing)}")
    if extra:
        errors.append(
            f"README MCP inventory contains unknown tools: {', '.join(extra)}"
        )
    if (
        "exposes 40 tools" not in readme
        or "Thirty tools return structured content" not in readme
    ):
        errors.append("README MCP totals do not match the checked wording")


def check_stale_claims(errors: list[str]) -> None:
    active = [
        ROOT / "README.md",
        ROOT / "docs/get-started/index.md",
        ROOT / "SECURITY.md",
        ROOT / "docs/blog/index.md",
    ]
    stale = ("2.12.1", "1,026 tests", "1026 tests", "18 structured", "13-rule")
    for path in active:
        text = path.read_text(encoding="utf-8").lower()
        for phrase in stale:
            if phrase.lower() in text:
                fail(errors, path, f"stale active-document claim: {phrase!r}")


def check_homepage_claims(errors: list[str]) -> None:
    path = ROOT / "docs/index.html"
    text = path.read_text(encoding="utf-8")
    if "40 MCP" not in text:
        fail(errors, path, "homepage must state the current 40-tool MCP surface")
    banned = (
        "24 MCP",
        "13 plugin",
        "64 actions",
        "921 tests",
        "1,026 tests",
        "15 crates",
    )
    lowered = text.lower()
    for phrase in banned:
        if phrase.lower() in lowered:
            fail(errors, path, f"stale homepage claim: {phrase!r}")


def check_cli_reference_claims(errors: list[str]) -> None:
    path = ROOT / "docs/guides/cli-reference.md"
    text = path.read_text(encoding="utf-8")
    for phrase in ("24 tools", "16 tools return structuredContent"):
        if phrase in text:
            fail(errors, path, f"stale MCP surface claim: {phrase!r}")


def check_blog_tool_counts(errors: list[str]) -> None:
    # Post 17 is explicitly labeled a historical 1.0-era retrospective and
    # post 20 documents the 24-tool surface "for version 2.1.0"; both are
    # versioned historical claims, so the present-tense ban exempts them.
    exempt = {"17-tumult-1.0.md", "20-mcp-first-class.md"}
    for path in sorted((ROOT / "docs/blog").glob("[0-9][0-9]-*.md")):
        if path.name in exempt:
            continue
        text = path.read_text(encoding="utf-8")
        for phrase in ("14 MCP tools", "All 14 MCP tools"):
            if phrase in text:
                fail(errors, path, f"stale MCP tool count: {phrase!r}")


def check_mermaid_accessibility(errors: list[str]) -> None:
    for path in DOC_FILES:
        text = path.read_text(encoding="utf-8")
        for block in re.findall(r"```mermaid\s*\n(.*?)```", text, re.S):
            if "accTitle:" not in block:
                fail(errors, path, "Mermaid diagram is missing accTitle")
            if "accDescr:" not in block:
                fail(errors, path, "Mermaid diagram is missing accDescr")


def main() -> int:
    errors: list[str] = []
    check_links(errors)
    check_blog(errors)
    check_mcp_inventory(errors)
    check_stale_claims(errors)
    check_homepage_claims(errors)
    check_cli_reference_claims(errors)
    check_blog_tool_counts(errors)
    check_mermaid_accessibility(errors)
    check_anchors(errors)
    check_repo_paths(errors)
    if errors:
        print("Documentation checks failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    print(
        "Documentation checks passed: local links and anchors, repository paths,"
        " 27-post index, and MCP inventory."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
