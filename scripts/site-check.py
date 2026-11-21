#!/usr/bin/env python3
"""Check a built docs site (the `_site` that Jekyll writes). See
.github/workflows/site-preview.yml.

Reads every html page of the site. Fails, with the page and the reason, on:
  raw-mermaid        a mermaid block on a page that loads no mermaid script
                     (a script whose src names mermaid, or an inline script
                     that imports it), so the reader sees the diagram source
                     as text
  liquid-left        `{{` or `{%` left in the text or an attribute of a page
                     (code, scripts and styles are not read)
  liquid-ate-braces  with --source: a `{{ }}` in a source page that Liquid
                     prints as nothing, such as the Go template `{{.event}}`
                     that shows as `line_format " "` (names the source line)
  broken-link        a link, image, script or style of the site that is not in
                     the site dir
  missing-page       with --baseline: a page of the baseline that is gone and
                     has no redirect, or that redirects to a missing page
  stale-known        a --known entry that matches no failure
  bad-known          a --known line that is not `page::rule  # reason`

A link is local when it has no host, or when its host is the one in the CNAME
file of the site. Local links resolve the way GitHub Pages serves them: `/a/`
is `a/index.html`, `/a` is `a`, `a.html` or `a/index.html`. Fragments are not
checked, nor `srcset`. A source page is an md file, or an html file with front
matter, whose built page is in the site (`a.md` is `a.html`, a `permalink:`
wins). GitHub Pages renders an md file with no front matter too.

--known lists the failures allowed for now, one `page::rule  # reason` per
line; the reason is required. An `overflow-<width>` entry belongs to
scripts/site-shots.mjs and is not read here.

Usage: scripts/site-check.py <site dir> [--source docs] [--known FILE]
       [--baseline FILE]
"""

from __future__ import annotations

import argparse
import re
import sys
from html.parser import HTMLParser
from pathlib import Path
from typing import NamedTuple
from urllib.parse import unquote, urljoin, urlsplit

RULES = {
    "raw-mermaid",
    "liquid-left",
    "liquid-ate-braces",
    "broken-link",
    "missing-page",
}
PICTURE_RULE = re.compile(r"overflow-\d+")
KNOWN_LINE = re.compile(r"(\S+)::([a-z0-9-]+)\s+#\s*(\S.*)")
LINK_ATTRS = {
    "a": ("href",),
    "area": ("href",),
    "link": ("href",),
    "img": ("src",),
    "script": ("src",),
    "source": ("src",),
    "track": ("src",),
    "iframe": ("src",),
    "embed": ("src",),
    "audio": ("src",),
    "video": ("src", "poster"),
}
NOT_READ = {"pre", "code", "script", "style", "textarea"}
LIQUID = re.compile(r"\{\{|\{%")
# `import mermaid from '.../mermaid.esm.min.mjs'`, as just-the-docs writes it.
MERMAID_IMPORT = re.compile(
    r"\bimport\b[^;'\"]*\bfrom\s*(['\"])[^'\"]*mermaid[^'\"]*\1", re.IGNORECASE
)
# Liquid prints these names; any other first name prints nothing.
JEKYLL_NAMES = {
    *("site", "page", "layout", "content", "paginator", "jekyll", "include"),
    *("forloop", "tablerowloop", "true", "false", "nil"),
}
SKIPPED_BLOCK = re.compile(
    r"\{%-?\s*(raw|comment)\s*-?%\}.*?\{%-?\s*end\1\s*-?%\}", re.DOTALL
)
OUTPUT_TAG = re.compile(r"\{\{-?(.*?)-?\}\}", re.DOTALL)
SET_NAME = re.compile(r"\{%-?\s*(?:assign|capture|for|increment|decrement)\s+(\w+)")
FIRST_NAME = re.compile(r"[A-Za-z_][\w-]*")
FRONT_MATTER = re.compile(r"---[ \t]*\r?\n(.*?\r?\n)?---[ \t]*\r?\n", re.DOTALL)
PERMALINK = re.compile(r"^permalink:\s*['\"]?([^'\"\s]+)", re.MULTILINE)
REFRESH_URL = re.compile(r"url\s*=\s*['\"]?([^'\"]+)", re.IGNORECASE)


class Failure(NamedTuple):
    page: str
    rule: str
    message: str


class Page(HTMLParser):
    """What the checks read from one html page."""

    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.links: list[str] = []
        self.text: list[str] = []
        self.mermaid_block = False
        self.mermaid_script = False
        self.redirect: str | None = None
        self._not_read = 0
        self._script: list[str] | None = None

    def handle_starttag(self, tag, attrs):
        values = {name: value or "" for name, value in attrs}
        self.text.extend(values.values())
        self.links.extend(values[a] for a in LINK_ATTRS.get(tag, ()) if a in values)
        if "language-mermaid" in values.get("class", "").split():
            self.mermaid_block = True
        if tag == "script" and "mermaid" in values.get("src", "").lower():
            self.mermaid_script = True
        if tag == "script" and "src" not in values:
            self._script = []
        if tag == "meta" and values.get("http-equiv", "").lower() == "refresh":
            found = REFRESH_URL.search(values.get("content", ""))
            self.redirect = found[1].strip() if found else None
        if tag in NOT_READ:
            self._not_read += 1

    def handle_endtag(self, tag):
        if tag in NOT_READ and self._not_read:
            self._not_read -= 1
        if tag == "script" and self._script is not None:
            if MERMAID_IMPORT.search("".join(self._script)):
                self.mermaid_script = True
            self._script = None

    def handle_data(self, data):
        if self._script is not None:
            self._script.append(data)
        if not self._not_read:
            self.text.append(data)


def read_page(path: Path) -> Page:
    parsed = Page()
    parsed.feed(path.read_text(encoding="utf-8", errors="replace"))
    parsed.close()
    return parsed


def site_host(site: Path) -> str | None:
    """The host the site is served at, from its CNAME file."""
    cname = site / "CNAME"
    if not cname.is_file():
        return None
    host = cname.read_text(encoding="utf-8").strip().lower()
    return host or None


def local_path(page: str, ref: str, host: str | None) -> str | None:
    """The decoded path of a link on `page`, or None when it leaves the site."""
    parts = urlsplit(urljoin("/" + page, ref.strip()))
    if parts.scheme not in ("", "http", "https"):
        return None
    if parts.netloc and parts.netloc.lower() != host:
        return None
    if parts.scheme and not parts.netloc:
        return None
    return unquote(parts.path) or "/"


def served(site: Path, path: str) -> bool:
    """True when GitHub Pages would answer `path` from this site dir."""
    root = site.resolve()
    target = (root / path.lstrip("/")).resolve()
    if target != root and root not in target.parents:
        return False
    if path.endswith("/"):
        return (target / "index.html").is_file()
    return (
        target.is_file()
        or (target / "index.html").is_file()
        or target.with_name(target.name + ".html").is_file()
    )


def page_failures(site: Path, name: str, parsed: Page, host: str | None):
    if parsed.mermaid_block and not parsed.mermaid_script:
        yield Failure(
            name,
            "raw-mermaid",
            "a mermaid block, but the page loads no mermaid script, so the "
            "diagram shows as text (set `mermaid:` in _config.yml)",
        )
    for text in parsed.text:
        for found in LIQUID.finditer(text):
            snippet = " ".join(text[found.start() :].split())[:40]
            yield Failure(name, "liquid-left", f"Liquid left in the page: `{snippet}`")
    broken = []
    for ref in parsed.links:
        path = local_path(name, ref, host)
        if path is not None and not served(site, path) and ref not in broken:
            broken.append(ref)
    for ref in broken:
        yield Failure(name, "broken-link", f"broken local link: {ref}")


def built_page(rel: Path, front: str) -> str:
    """The site path Jekyll writes a source page to."""
    permalink = PERMALINK.search(front)
    if permalink:
        path = permalink[1].lstrip("/")
        return path + "index.html" if path.endswith("/") or not path else path
    return rel.with_suffix(".html").as_posix()


def blank_out(match: re.Match) -> str:
    """The same number of lines, so line numbers stay right."""
    return "\n" * match[0].count("\n")


def eaten_braces(text: str):
    """(line, tag) for each `{{ }}` that Liquid prints as nothing."""
    text = SKIPPED_BLOCK.sub(blank_out, text)
    known = JEKYLL_NAMES | set(SET_NAME.findall(text))
    for tag in OUTPUT_TAG.finditer(text):
        expression = tag[1].strip()
        if expression[:1] in ("'", '"') or expression[:1].isdigit():
            continue
        name = FIRST_NAME.match(expression)
        if name and name[0] in known:
            continue
        yield text.count("\n", 0, tag.start()) + 1, " ".join(tag[0].split())


def source_failures(site: Path, source: Path):
    for path in sorted(source.rglob("*")):
        rel = path.relative_to(source)
        if path.suffix not in (".md", ".markdown", ".html") or not path.is_file():
            continue
        if any(part.startswith(("_", ".")) for part in rel.parts):
            continue
        text = path.read_text(encoding="utf-8", errors="replace")
        front = FRONT_MATTER.match(text)
        if not front and path.suffix == ".html":
            continue
        name = built_page(rel, (front[1] or "") if front else "")
        if not (site / name).is_file():
            continue
        for line, tag in eaten_braces(text):
            yield Failure(
                name,
                "liquid-ate-braces",
                f"{(source / rel).as_posix()}:{line}: Liquid prints nothing for "
                f"`{tag}`; put it between {{% raw %}} and {{% endraw %}}",
            )


def baseline_failures(site: Path, baseline, host: str | None):
    for name in baseline:
        path = site / name
        if not path.is_file():
            yield Failure(
                name,
                "missing-page",
                "in the baseline, but the site has no page or redirect for it",
            )
            continue
        target = read_page(path).redirect
        local = local_path(name, target, host) if target else None
        if local is not None and not served(site, local):
            yield Failure(
                name, "missing-page", f"redirects to a missing page: {target}"
            )


def check_site(site: Path, source: Path | None = None, baseline=()) -> list[Failure]:
    """Every failure of the site, page by page, then the source and baseline."""
    host = site_host(site)
    failures = []
    for path in sorted(site.rglob("*.html")):
        name = path.relative_to(site).as_posix()
        failures.extend(page_failures(site, name, read_page(path), host))
    if source is not None:
        failures.extend(source_failures(site, source))
    failures.extend(baseline_failures(site, baseline, host))
    return failures


def read_baseline(text: str) -> list[str]:
    """The site paths of a baseline file: one per line, `#` starts a comment."""
    rows = (row.split("#", 1)[0].strip() for row in text.splitlines())
    return [row for row in rows if row]


def read_known(text: str, name: str):
    """({(page, rule): line number}, problems) of a known file."""
    entries, problems = {}, []
    for number, row in enumerate(text.splitlines(), 1):
        row = row.strip()
        if not row or row.startswith("#"):
            continue
        entry = KNOWN_LINE.fullmatch(row)
        where = f"{name}:{number}"
        if not entry:
            problems.append(
                Failure(where, "bad-known", f"not `page::rule  # reason`: {row}")
            )
        elif entry[2] not in RULES and not PICTURE_RULE.fullmatch(entry[2]):
            problems.append(Failure(where, "bad-known", f"unknown rule: {entry[2]}"))
        elif entry[2] in RULES:
            entries[(entry[1], entry[2])] = number
    return entries, problems


def split_known(failures, text: str, name: str):
    """(failures that fail the check, failures the known file allows)."""
    entries, problems = read_known(text, name)
    new = [f for f in failures if (f.page, f.rule) not in entries]
    allowed = [f for f in failures if (f.page, f.rule) in entries]
    used = {(f.page, f.rule) for f in allowed}
    for (page, rule), number in entries.items():
        if (page, rule) not in used:
            message = f"`{page}::{rule}` matches no failure; delete the line"
            problems.append(Failure(f"{name}:{number}", "stale-known", message))
    return new + problems, allowed


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("site", type=Path, help="the built site dir")
    parser.add_argument("--source", type=Path, help="the source dir (docs)")
    parser.add_argument("--known", type=Path, help="failures allowed for now")
    parser.add_argument("--baseline", type=Path, help="pages that must stay")
    args = parser.parse_args(argv)
    for path in (args.site, args.source):
        if path is not None and not path.is_dir():
            print(f"site-check: not a dir: {path}", file=sys.stderr)
            return 2
    for path in (args.known, args.baseline):
        if path is not None and not path.is_file():
            print(f"site-check: not a file: {path}", file=sys.stderr)
            return 2
    baseline = read_baseline(args.baseline.read_text("utf-8")) if args.baseline else []
    failures = check_site(args.site, args.source, baseline)
    known = args.known.read_text("utf-8") if args.known else ""
    new, allowed = split_known(failures, known, str(args.known))
    for failure in allowed:
        print(f"known: {failure.page}: {failure.rule}: {failure.message}")
    for failure in new:
        print(f"{failure.page}: {failure.rule}: {failure.message}")
    pages = len(list(args.site.rglob("*.html")))
    if new:
        print(f"{len(new)} site failure(s) in {pages} pages.")
        return 1
    print(f"Site check passed: {pages} pages, {len(allowed)} known failure(s).")
    return 0


if __name__ == "__main__":
    sys.exit(main())
