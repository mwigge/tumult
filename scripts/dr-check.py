#!/usr/bin/env python3
"""DR assertions against the Krönika daemon the DR examples reported to.

`scripts/demo-check.sh` runs the examples of `examples/` in the demo with
the daemon of its own Krönika stack as their daemon, and then calls this:

    scripts/dr-check.py URL --expectations examples/expectations.json \\
        --journals DIR --function cif-0001 [--wait 30]

The API token is read from TUMULT_DAEMON_TOKEN. The DR examples are the
examples the expectations give to the proof `demo-check` whose journal in
DIR is of kind `dr`. Each journal names the run, its target and the plan
scenarios it tested. The check holds when:

- `GET /api/dr` of the target and the environment of each DR example
  holds its run as a DR test that is met and recovered, with its measured
  RPO in the range the expectations name;
- `GET /dr` answers the app shell of the web UI. The page is built in the
  browser, so the shell alone proves little: the route module that
  `/dr.html` loads must hold the heading of the DR page. This proves the
  page is in the build the daemon serves, not what the page shows;
- the DR test report (`dr-test`) of the target of each DR example that
  measured an RPO names its run, with that RPO and its verdict;
- the DORA evidence pack holds the row of RTS 2024/1774 Art. 26;
- `GET /api/dora` gives each plan scenario a DR example names the state
  `passed` for the function FUNCTION, with that run as its last test.

The DR view and the DORA score are read again until they hold or `--wait`
seconds have passed: the score is served from a cache for a few seconds.

Each line starts with `ok  ` or `FAIL`. Exit 0 when no line fails, 1
otherwise.
"""

import argparse
import functools
import importlib.util
import json
import os
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from html.parser import HTMLParser
from pathlib import Path

TIMEOUT_S = 60
POLL_S = 1.0
PROOF = "demo-check"
JOURNAL_CHECK = Path(__file__).with_name("check-proof-journals.py")
SHELL_MARK = "/_app/immutable/entry/start."
MODULE = re.compile(r'href="\.?(/_app/immutable/nodes/[^"]+\.js)"')
PAGE_HEADING = "Disaster recovery"
ART26 = "RTS 2024/1774 Art. 26"
# A status cell of a report starts with a glyph of its tone.
GLYPHS = "●▲×○ "
# The RPO a report prints is rounded to a tenth of a second.
RPO_PLACES = 0.05 + 1e-9
# What a daemon that is absent, or one that answers something other than
# the documented answer, raises somewhere in a check.
UNEXPECTED = (
    urllib.error.URLError,
    OSError,
    ValueError,
    LookupError,
    TypeError,
    AttributeError,
)
MARK = {True: "ok  ", False: "FAIL"}


@functools.cache
def journal_check():
    """check-proof-journals.py, whose reader reads the journals the CLI writes."""
    spec = importlib.util.spec_from_file_location("check_proof_journals", JOURNAL_CHECK)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class DrRun:
    """A run of a DR example, as its journal and its expectations give it."""

    def __init__(self, name, journal, expect):
        objectives = journal.get("objectives") or {}
        target = journal.get("target") or {}
        regulatory = journal.get("regulatory") or {}
        self.name = name
        self.id = journal["experiment_id"]
        self.title = journal["experiment_title"]
        self.system = target.get("system")
        self.environment = target.get("environment")
        self.rpo_s = objectives.get("rpo_measured_s")
        self.rpo_range = expect.get("rpo_measured_s")
        self.scenarios = regulatory.get("scenarios") or []

    @property
    def target(self):
        return self.system, self.environment


def dr_runs(expectations, journals):
    """The runs of the DR examples, and a failed line for each example of
    the proof that left no journal and each DR run that names no target."""
    entries = json.loads(Path(expectations).read_text())["experiments"]
    runs, lines = [], []
    for name, entry in entries.items():
        if entry.get("proof") != PROOF:
            continue
        path = Path(journals) / f"{name.removesuffix('.toon')}.journal.toon"
        if not path.is_file():
            lines.append((False, f"dr: no journal of {name} to read"))
            continue
        journal = journal_check().read_toon(path.read_text())
        if journal.get("kind") != "dr":
            continue
        run = DrRun(name, journal, entry.get("expect") or {})
        if run.system is None:
            lines.append((False, f"dr: the journal of {name} names no target"))
            continue
        runs.append(run)
    if not runs and not lines:
        lines.append((False, f"dr: no example of the proof {PROOF} is of kind dr"))
    return runs, lines


def call(base, path, token, body=None):
    """One request: the status and the body of its answer. A read asks for
    a fresh answer."""
    headers = {"Cache-Control": "no-cache"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    data = None
    if body is not None:
        headers["Content-Type"] = "application/json"
        data = json.dumps(body).encode()
    request = urllib.request.Request(base + path, data=data, headers=headers)
    try:
        with urllib.request.urlopen(request, timeout=TIMEOUT_S) as response:
            return response.status, response.read().decode()
    except urllib.error.HTTPError as error:
        return error.code, error.read().decode()


def seconds(value):
    return "none" if value is None else f"{value:.1f} s"


def is_number(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def targets_of(runs):
    """The runs by target, in the order the runs come."""
    by_target = {}
    for run in runs:
        by_target.setdefault(run.target, []).append(run)
    return by_target


def where(target):
    return f"{target[0]} in {target[1]}"


def dr_query(target):
    query = urllib.parse.urlencode({"target": target[0], "environment": target[1]})
    return f"/api/dr?{query}"


def dr_test_line(run, tests):
    """Whether the DR view holds the run as a met test that recovered, with
    its RPO in the range of its expectations."""
    test = next((t for t in tests if t.get("test_id") == f"automated:{run.id}"), None)
    if test is None:
        return False, (
            f"dr: {run.name}: run {run.id} is not among the {len(tests)} DR tests "
            f"of {where(run.target)}"
        )
    holds = test["verdict"] == "met" and test["recovered"] is True
    facts = (
        f"verdict {test['verdict']}, recovered {json.dumps(test['recovered'])}, "
        f"RTO {seconds(test['rto_measured_s'])} of {seconds(test['rto_target_s'])}"
    )
    if run.rpo_range is not None:
        rpo = test["rpo_measured_s"]
        low, high = run.rpo_range["from"], run.rpo_range["to"]
        holds = holds and is_number(rpo) and low <= rpo <= high
        facts += f", RPO {seconds(rpo)} (expected {low} to {high})"
    return holds, f"dr: {run.name} is a DR test of {where(run.target)}: {facts}"


def dr_lines(base, token, runs):
    """A line for each DR run: what the DR view of its target says of it."""
    lines = []
    for target, held in targets_of(runs).items():
        status, text = call(base, dr_query(target), token)
        if status != 200:
            lines.append((False, f"dr: GET {dr_query(target)} answered {status}"))
            continue
        tests = json.loads(text)["tests"]
        lines += [dr_test_line(run, tests) for run in held]
    return lines


def page_line(base):
    """Whether the daemon serves the app shell at /dr and the DR page in its
    build. The page is read as a browser reads it, with no token."""
    status, shell = call(base, "/dr", None)
    if status != 200 or SHELL_MARK not in shell:
        return False, f"dr page: GET /dr answered {status} without the app shell"
    status, page = call(base, "/dr.html", None)
    loaded = set(MODULE.findall(shell))
    modules = (
        [m for m in MODULE.findall(page) if m not in loaded] if status == 200 else []
    )
    for module in modules:
        status, code = call(base, module, None)
        if status == 200 and PAGE_HEADING in code:
            return True, (
                "dr page: GET /dr answered 200 with the app shell, which builds the page "
                f"in the browser; its route module {module} holds the heading {PAGE_HEADING}"
            )
    return False, (
        f"dr page: no route module of /dr.html holds the heading {PAGE_HEADING} "
        f"({len(modules)} read)"
    )


class Tables(HTMLParser):
    """The tables of a report: each a list of rows, each row the text of
    its cells, a status cell without its glyph."""

    def __init__(self, text):
        super().__init__()
        self.tables, self.row, self.cell = [], None, None
        self.feed(text)
        self.close()

    def handle_starttag(self, tag, attrs):
        if tag == "table":
            self.tables.append([])
        elif tag == "tr" and self.tables:
            self.row = []
            self.tables[-1].append(self.row)
        elif tag in ("td", "th") and self.row is not None:
            self.cell = []

    def handle_endtag(self, tag):
        if tag in ("td", "th") and self.cell is not None:
            text = " ".join("".join(self.cell).split())
            self.row.append(text.lstrip(GLYPHS))
            self.cell = None
        elif tag == "tr":
            self.row = None

    def handle_data(self, data):
        if self.cell is not None:
            self.cell.append(data)

    def rows_under(self, header):
        """The rows of the first table with `header`, by column name."""
        for table in self.tables:
            if table and header in table[0]:
                return [dict(zip(table[0], row)) for row in table[1:]]
        return []

    def has_row(self, *cells):
        return any(row == list(cells) for table in self.tables for row in table)


def generate(base, token, request):
    """Build one report through the API: its document id and its tables, or
    the line that says why there are none."""
    status, text = call(base, "/api/reports/v2/generate", token, request)
    if status != 200:
        return None, f"POST /api/reports/v2/generate answered {status}"
    doc = json.loads(text)["doc_id"]
    status, page = call(base, f"/api/reports/v2/{doc}/html", token)
    if status != 200:
        return None, f"GET /api/reports/v2/{doc}/html answered {status}"
    return (doc, Tables(page)), None


def report_line(doc, tables, run):
    """Whether the DR test report names the run with its RPO and verdict."""
    named = f"{run.title} (automated)"
    row = next(
        (r for r in tables.rows_under("RPO actual") if r.get("Test") == named), None
    )
    if row is None:
        return False, f"dr report {doc}: no row of its DR tests names {named}"
    actual, verdict = row["RPO actual"], row["Verdict"]
    shown = actual.removesuffix(" s")
    near = (
        shown.replace(".", "", 1).isdigit()
        and abs(float(shown) - run.rpo_s) <= RPO_PLACES
    )
    cited = tables.has_row("Run id", run.id)
    evidence = "names" if cited else "does not name"
    text = (
        f"dr report {doc}: the row of {run.title} reads RPO actual {actual} and verdict "
        f"{verdict}, the run measured {seconds(run.rpo_s)}, and its evidence "
        f"{evidence} run {run.id}"
    )
    return near and verdict == "met" and cited, text


def report_lines(base, token, runs):
    """A line for each DR run that measured an RPO: what the DR test report
    of its target says of it."""
    measured = [run for run in runs if is_number(run.rpo_s)]
    if not measured:
        return [(False, "dr report: no DR example measured an RPO")]
    lines = []
    for target, held in targets_of(measured).items():
        request = {"type": "dr-test", "target": target[0], "environment": target[1]}
        built, why = generate(base, token, request)
        if built is None:
            lines.append((False, f"dr report of {where(target)}: {why}"))
            continue
        lines += [report_line(*built, run) for run in held]
    return lines


def pack_line(base, token):
    """Whether the DORA evidence pack holds the row of RTS 2024/1774 Art. 26."""
    built, why = generate(base, token, {"type": "evidence-pack", "framework": "dora"})
    if built is None:
        return False, f"evidence pack: {why}"
    doc, tables = built
    rows = tables.rows_under("Article and paragraph")
    row = next((r for r in rows if r["Article and paragraph"] == ART26), None)
    if row is None:
        return False, f"evidence pack {doc}: its DORA matrix has no row {ART26}"
    return True, (
        f"evidence pack {doc}: its DORA matrix holds the row {ART26}: "
        f"{row['Status']}, {row['Evidence']}"
    )


def scenario_line(functions, function, run, scenario):
    """Whether the function passed the plan scenario with the run as its
    last test."""
    tracked = [f for f in functions if f["entity_id"] == function]
    cell = next(
        (s for f in tracked for s in f["scenarios"] if s["id"] == scenario), None
    )
    if cell is None:
        return False, f"dora: {function} tracks no plan scenario {scenario}"
    last = cell.get("last_test") or {}
    tested = f"{last['source']} {last['id']}" if last else "nothing"
    named = f" ({tracked[0]['name']})" if tracked[0].get("name") else ""
    text = f"dora: {scenario} of {function}{named} is {cell['state']}, last tested by {tested}"
    holds = (
        cell["state"] == "passed"
        and last.get("source") == "run"
        and last.get("id") == run.id
    )
    if holds:
        return True, f"{text} of {run.name}"
    return False, f"{text}; the run of {run.name} is {run.id}"


def dora_lines(base, token, runs, function):
    """A line for each plan scenario a DR run names: its state for the
    function in the DORA score."""
    named = [(run, scenario) for run in runs for scenario in run.scenarios]
    if not named:
        return [(False, "dora: no DR example names a plan scenario")]
    status, text = call(base, "/api/dora", token)
    if status != 200:
        return [(False, f"dora: GET /api/dora answered {status}")]
    functions = json.loads(text)["scenarios"]["functions"]
    return [scenario_line(functions, function, *pair) for pair in named]


def settled(read, wait_s):
    """Read until every line holds or the wait is over."""
    deadline = time.monotonic() + wait_s
    while True:
        lines = read()
        if all(holds for holds, _ in lines) or time.monotonic() >= deadline:
            return lines
        time.sleep(POLL_S)


def guarded(label, base, check):
    """The lines of one check, or one failed line when the daemon did not
    answer as expected."""
    try:
        return check()
    except UNEXPECTED as error:
        return [(False, f"{label}: {base} did not answer as expected: {error!r}")]


def checked(base, token, args):
    """Every line of the check: (holds, text)."""
    runs, lines = dr_runs(args.expectations, args.journals)
    views = functools.partial(dr_lines, base, token, runs)
    scores = functools.partial(dora_lines, base, token, runs, args.function)
    if runs:
        lines += guarded("dr", base, lambda: settled(views, args.wait))
    lines += guarded("dr page", base, lambda: [page_line(base)])
    lines += guarded("dr report", base, lambda: report_lines(base, token, runs))
    lines += guarded("evidence pack", base, lambda: [pack_line(base, token)])
    lines += guarded("dora", base, lambda: settled(scores, args.wait))
    return lines


def main():
    parser = argparse.ArgumentParser(
        description="DR assertions for the DR examples of the demo."
    )
    parser.add_argument(
        "url", help="base URL of the daemon, e.g. http://127.0.0.1:24318"
    )
    parser.add_argument(
        "--expectations", required=True, help="examples/expectations.json"
    )
    parser.add_argument(
        "--journals", required=True, help="the journals of the examples"
    )
    parser.add_argument(
        "--function", required=True, help="the function that binds the DR target"
    )
    parser.add_argument(
        "--wait", type=float, default=30.0, help="seconds to wait for a read to hold"
    )
    args = parser.parse_args()
    base = args.url.rstrip("/")
    token = os.environ.get("TUMULT_DAEMON_TOKEN", "")
    lines = checked(base, token, args)
    for holds, text in lines:
        print(f"{MARK[holds]} {text}")
    return 1 if any(not holds for holds, _ in lines) else 0


if __name__ == "__main__":
    sys.exit(main())
