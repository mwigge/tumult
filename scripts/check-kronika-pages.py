#!/usr/bin/env python3
"""Check that docs/reference/kronika-pages.md says what the web UI does.

Fails, with the block of the page and the reason, on:
  missing      a page calls an API endpoint that its block does not list
  not-called   a block lists an endpoint that its page does not call
  no-route     a block names a route with no folder under web/src/routes
  no-blocks    the page has no `### Page` block with a route
  no-endpoint  a function of the client fetches no endpoint the check can read
  layout-call  the layout makes a call that the text before the first `##`
               heading does not name
  colour       the "Colours of a run" table and RUN_TONE of
               web/src/lib/tone.ts disagree on a run ending

A block is a `### Page` heading with a line that starts with `Route` or
`Routes` and names the routes in backticks, up to the first `.` or `;`, and
a `**API:**` bullet that names the endpoints as `METHOD /api/path`. Blocks on
one route folder (`/scores` and `/scores?view=dora`) are checked as one.

The calls of a route are the `api.<name>(` calls of the files of its folder
and of every file they import through `$lib/`, `#lib/` or a relative path.
web/src/lib/api.ts says which endpoints each client function fetches. Test
files are left out. The calls of the layout are made on every page, so the
blocks do not list them.

In the colour table, ok is Green, fail is Red, warn is Amber and neutral is
Grey, and every run ending of RUN_TONE is in exactly one row.

Usage: scripts/check-kronika-pages.py [--root DIR]
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PAGE = "docs/reference/kronika-pages.md"
LIB = "web/src/lib"
ROUTES = "web/src/routes"
TONE_COLOUR = {"ok": "Green", "fail": "Red", "warn": "Amber", "neutral": "Grey"}

CLIENT_FUNCTION = re.compile(
    r"^ {4}(\w+): (?:async )?\((.*?)(?=^ {4}\w+: (?:async )?\(|^\};)", re.S | re.M
)
CLIENT_PATH = re.compile(
    r"""\(\s*(?:'([A-Z]+)',\s*)?[`']((?:/api|/report)[^`']*)[`']"""
)
FETCH_METHOD = re.compile(r"method:\s*'([A-Z]+)'")
IMPORT = re.compile(r"""(?:from|import)\s+['"]([$#]lib/[^'"]+|\.{1,2}/[^'"]+)['"]""")
API_CALL = re.compile(r"\bapi\.(\w+)\(")
ROUTE_LINE = re.compile(r"^Routes? (.*?)[.;]", re.M)
DOC_ENDPOINT = re.compile(r"`((?:GET|POST|PUT|PATCH|DELETE) /api/[^`]*)`")
COLOUR_ROW = re.compile(r"^\| (\w+) \| ([^|]*) \|", re.M)


def normalize(endpoint: str) -> str:
    """`METHOD /path` with every path parameter written `{}`."""
    return re.sub(r"\{[^}]*\}", "{}", endpoint.split("?", 1)[0]).rstrip("/")


def client_endpoints(api_text: str) -> dict[str, set[str]]:
    """The endpoints each function of the `api` client fetches."""
    client = api_text.split("export const api = {", 1)[-1]
    out: dict[str, set[str]] = {}
    for name, body in CLIENT_FUNCTION.findall(client):
        fetch_method = FETCH_METHOD.search(body) if "fetch(" in body else None
        endpoints = set()
        for method, path in CLIENT_PATH.findall(body):
            path = re.sub(r"(?<=/)\$\{[^}]*\}", "{}", path)
            path = re.split(r"\$\{|\?", path)[0]
            method = method or (fetch_method[1] if fetch_method else "GET")
            endpoints.add(normalize(f"{method} {path}"))
        out[name] = endpoints
    return out


def resolve_import(root: Path, file: Path, spec: str) -> Path | None:
    """The file an import of `file` names, or None when it is not ours."""
    if spec.startswith(("$lib/", "#lib/")):
        base = root / LIB / spec[5:]
    else:
        base = file.parent / spec
    candidates = [base, base.with_suffix(".ts"), base.with_suffix(".svelte")]
    candidates.append(base / "index.ts")
    if base.suffix == ".js":
        candidates.insert(0, base.with_suffix(".ts"))
    for candidate in candidates:
        if candidate.is_file():
            return candidate.resolve()
    return None


def reachable(root: Path, start: list[Path]) -> set[Path]:
    """`start` and every file it imports, directly or not, but the client."""
    seen: set[Path] = set()
    todo = list(start)
    while todo:
        file = todo.pop()
        if file in seen or ".test." in file.name:
            continue
        seen.add(file)
        for spec in IMPORT.findall(file.read_text(encoding="utf-8")):
            found = resolve_import(root, file, spec)
            if found and found != (root / LIB / "api.ts").resolve():
                todo.append(found)
    return seen


def calls(root: Path, start: list[Path], client: dict[str, set[str]]) -> set[str]:
    """The endpoints the files of `start` call through the client."""
    out: set[str] = set()
    for file in reachable(root, start):
        for name in API_CALL.findall(file.read_text(encoding="utf-8")):
            out |= client.get(name, set())
    return out


def route_folder(root: Path, route: str) -> Path | None:
    """The folder of a route, `{id}` read as the one `[param]` folder."""
    folder = root / ROUTES
    for part in route.split("?", 1)[0].strip("/").split("/"):
        if not part:
            continue
        if part.startswith("{"):
            params = sorted(folder.glob("[[]*[]]"))
            folder = params[0] if len(params) == 1 else folder / part
        else:
            folder = folder / part
    return folder if folder.is_dir() else None


def route_files(folder: Path) -> list[Path]:
    """The files of one route folder, without the folders below it."""
    return [
        file.resolve()
        for file in sorted(folder.iterdir())
        if file.suffix in (".svelte", ".ts")
    ]


def blocks(page: str) -> dict[str, tuple[list[str], set[str]]]:
    """Each page block as name: (routes, listed endpoints). Blocks on one
    route folder are joined under `A + B`."""
    found: dict[str, tuple[list[str], set[str]]] = {}
    for block in re.split(r"^### ", page, flags=re.M)[1:]:
        block = re.split(r"^## ", block, flags=re.M)[0]
        route_line = ROUTE_LINE.search(block)
        if not route_line:
            continue
        name = block.splitlines()[0].strip()
        routes = re.findall(r"`(/[^`]*)`", route_line[1])
        api = block.split("**API:**", 1)[1] if "**API:**" in block else ""
        found[name] = (routes, {normalize(e) for e in DOC_ENDPOINT.findall(api)})
    joined: dict[tuple[str, ...], list[str]] = {}
    for name, (routes, _) in found.items():
        key = tuple(sorted({route.split("?", 1)[0] for route in routes}))
        joined.setdefault(key, []).append(name)
    out = {}
    for names in joined.values():
        routes = [route for name in names for route in found[name][0]]
        listed = set().union(*(found[name][1] for name in names))
        out[" + ".join(names)] = (routes, listed)
    return out


def check_calls(root: Path, page: str) -> list[tuple[str, str, str]]:
    client = client_endpoints((root / LIB / "api.ts").read_text(encoding="utf-8"))
    layout = sorted((root / ROUTES).glob("+layout.*"))
    shared = calls(root, [file.resolve() for file in layout], client)
    intro = re.split(r"^## ", page, maxsplit=1, flags=re.M)[0]
    named = {normalize(e) for e in DOC_ENDPOINT.findall(intro)}
    failures = [("api.ts", "no-endpoint", f) for f, e in client.items() if not e]
    failures += [("intro", "layout-call", e) for e in sorted(shared - named)]
    found = blocks(page)
    if not found:
        failures.append(("pages", "no-blocks", "no `### Page` with a route"))
    for name, (routes, listed) in found.items():
        start = []
        for route in routes:
            folder = route_folder(root, route)
            if folder is None:
                failures.append((name, "no-route", route))
                continue
            start += route_files(folder)
        called = calls(root, start, client) - shared
        failures += [(name, "missing", e) for e in sorted(called - listed)]
        failures += [(name, "not-called", e) for e in sorted(listed - called)]
    return failures


def check_colours(root: Path, page: str) -> list[tuple[str, str, str]]:
    tone = (root / LIB / "tone.ts").read_text(encoding="utf-8")
    table = tone.split("RUN_TONE", 1)[-1].split("};", 1)[0]
    code = {
        status: TONE_COLOUR.get(t, t)
        for status, t in re.findall(r"(\w+): '(\w+)'", table)
    }
    section = page.split("## Colours of a run", 1)[-1].split("\n## ", 1)[0]
    documented: dict[str, str] = {}
    problems = []
    for colour, cell in COLOUR_ROW.findall(section):
        for status in re.findall(r"`(\w+)`", cell):
            if status in documented:
                problems.append(f"{status}: in more than one row")
            documented[status] = colour
    if not documented:
        return [
            ("Colours of a run", "colour", "no row of the table names a run ending")
        ]
    for status, colour in code.items():
        if status not in documented:
            problems.append(f"{status}: {colour} in tone.ts, not on the page")
        elif documented[status] != colour:
            problems.append(
                f"{status}: {colour} in tone.ts, {documented[status]} on the page"
            )
    problems += [
        f"{status}: on the page, not in tone.ts"
        for status in documented
        if status not in code
    ]
    return [("Colours of a run", "colour", problem) for problem in problems]


def check(root: Path) -> list[tuple[str, str, str]]:
    """Every failure as (block, rule, detail)."""
    page = (root / PAGE).read_text(encoding="utf-8")
    return check_colours(root, page) + check_calls(root, page)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--root", type=Path, default=ROOT)
    args = parser.parse_args()
    failures = check(args.root)
    for block, rule, detail in failures:
        sys.stdout.write(f"{block}: {rule}: {detail}\n")
    if failures:
        sys.stdout.write(f"{len(failures)} failure(s) in {PAGE}\n")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
