#!/usr/bin/env python3
"""Check documentation links and repository-derived inventory claims."""

from __future__ import annotations

import re
import sys
from pathlib import Path
from urllib.parse import unquote

ROOT = Path(__file__).resolve().parents[1]
DOC_FILES = [
    ROOT / name for name in ("README.md", "QUICKSTART.md", "SECURITY.md", "CHANGELOG.md")
] + sorted((ROOT / "docs").rglob("*.md"))


def fail(errors: list[str], path: Path, message: str) -> None:
    errors.append(f"{path.relative_to(ROOT)}: {message}")


def check_links(errors: list[str]) -> None:
    link_re = re.compile(r"(?<!!)\[[^]]*\]\(([^)]+)\)")
    for path in DOC_FILES:
        text = path.read_text(encoding="utf-8")
        for raw in link_re.findall(text):
            target = raw.strip().split(maxsplit=1)[0].strip("<>")
            if not target or "{{" in target or "{%" in target or target.startswith(("#", "http://", "https://", "mailto:", "tumult://")):
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
    source = "\n".join(path.read_text(encoding="utf-8") for path in schema_dir.glob("*.rs"))
    tools = set(re.findall(r'name\s*=\s*"(tumult_[a-z0-9_]+)"', source))
    if len(tools) != 40:
        errors.append(f"MCP schemas: expected 40 named tools, found {len(tools)}")

    output = (ROOT / "tumult-mcp/src/handler/output_schema.rs").read_text(encoding="utf-8")
    match = re.search(r"STRUCTURED_TOOLS:\s*&\[&str\]\s*=\s*&\[(.*?)\];", output, re.S)
    structured = set(re.findall(r'"(tumult_[a-z0-9_]+)"', match.group(1))) if match else set()
    if len(structured) != 30:
        errors.append(f"structured MCP schemas: expected 30 tools, found {len(structured)}")

    readme = (ROOT / "README.md").read_text(encoding="utf-8")
    documented = set(re.findall(r"`(tumult_[a-z0-9_]+)`", readme))
    missing = sorted(tools - documented)
    extra = sorted(documented - tools)
    if missing:
        errors.append(f"README MCP inventory missing: {', '.join(missing)}")
    if extra:
        errors.append(f"README MCP inventory contains unknown tools: {', '.join(extra)}")
    if "exposes 40 tools" not in readme or "Thirty tools return structured content" not in readme:
        errors.append("README MCP totals do not match the checked wording")


def check_stale_claims(errors: list[str]) -> None:
    active = [ROOT / "README.md", ROOT / "QUICKSTART.md", ROOT / "SECURITY.md", ROOT / "docs/blog/index.md"]
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
    banned = ("24 MCP", "13 plugin", "64 actions", "921 tests", "1,026 tests", "15 crates")
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
    if errors:
        print("Documentation checks failed:", file=sys.stderr)
        for error in errors:
            print(f"- {error}", file=sys.stderr)
        return 1
    print("Documentation checks passed: local links, 27-post index, and MCP inventory.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
