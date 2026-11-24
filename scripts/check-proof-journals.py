#!/usr/bin/env python3
"""Compare CLI-generated TOON journals with the expectations of their set.

Each experiment set has an `expectations.json` that names, per experiment,
the proof that runs it and what its journal must say: status, hypothesis met
before and after, the number of rollbacks and the state of each objective.
A load result that is expected must hold requests and a p99 above zero, and
the load must have run the seconds the definition next to the file plans.
A GameDay entry can also name the score its journal must hold: the status
and the recovery part, to two decimals. An entry can name the seconds of
data its run must have lost, `rpo_measured_s` as `{"from": a, "to": b}`:
the RPO the run measured must lie in that range, both ends in it.

    check-proof-journals.py --proof NAME EXPECTATIONS JOURNAL_DIR
    check-proof-journals.py --proof NAME --list EXPECTATIONS
    check-proof-journals.py --describe JOURNAL

The journal of `x.toon` is `JOURNAL_DIR/x.journal.toon`. A GameDay file sits
next to its expectations file; its journal holds one record per experiment.
The reader below reads the layout the CLI writes, not every TOON document.
"""

import argparse
import json
import re
import sys
from pathlib import Path

INDENT = 2
KEY_LINE = re.compile(
    r"""(?:"((?:[^"\\]|\\.)*)"|([^\s:\[{"]+))(?:\[(\d+)\])?(?:\{([^}]*)\})?:(?: (.*))?"""
)
CELL = re.compile(r"""(?:"(?:[^"\\]|\\.)*"|[^,])+""")
NUMBER = re.compile(r"-?(?:0|[1-9]\d*)(\.\d+)?([eE][+-]?\d+)?")
WORDS = {"true": True, "false": False, "null": None}
FACTS = ("status", "hypothesis_before", "hypothesis_after", "rollbacks", "objectives")
MEASURED = ("total_requests", "latency_p99_ms")
COMPARED = (
    "status",
    "hypothesis_before",
    "hypothesis_after",
    "rollbacks",
    "rollback_failures",
)
SCORE = ("status", "recovery_compliance")


class Lines:
    """The lines of a document that are not blank, read one at a time."""

    def __init__(self, text):
        numbered = enumerate(text.splitlines(), start=1)
        self.rows = [
            (len(raw) - len(raw.lstrip(" ")), raw.strip(), number)
            for number, raw in numbered
            if raw.strip()
        ]
        self.at = 0

    def indent(self):
        """The indent of the next line, or -1 at the end."""
        return self.rows[self.at][0] if self.at < len(self.rows) else -1

    def content(self):
        return self.rows[self.at][1]

    def take(self):
        row = self.rows[self.at]
        self.at += 1
        return row


def scalar(text):
    """One value: a quoted string, a number, true, false, null or plain text."""
    text = text.strip()
    if text.startswith('"'):
        return json.loads(text)
    if text in WORDS:
        return WORDS[text]
    number = NUMBER.fullmatch(text)
    if not number:
        return text
    return float(text) if number[1] or number[2] else int(text)


def cells(text):
    """The comma-separated values of a row. A comma inside quotes is text."""
    return [scalar(cell) for cell in CELL.findall(text)]


def read_toon(text):
    """The document as dicts and lists. Raises ValueError with a line number."""
    lines = Lines(text)
    document = read_object(lines, 0, {})
    if lines.indent() != -1:
        raise ValueError(f"line {lines.take()[2]}: unexpected indent")
    return document


def read_object(lines, indent, into):
    """Read the `key:` lines at `indent` into `into`."""
    while lines.indent() == indent:
        _, content, number = lines.take()
        read_field(lines, indent, content, number, into)
    return into


def read_field(lines, indent, content, number, into):
    """Read one `key:` line, and the lines below that belong to it."""
    match = KEY_LINE.fullmatch(content)
    if not match:
        raise ValueError(f"line {number}: not a key line: {content}")
    quoted, plain, length, fields, rest = match.groups()
    key = plain if quoted is None else json.loads(f'"{quoted}"')
    if length is None:
        into[key] = read_plain(lines, indent, rest)
        return
    values = read_list(lines, indent + INDENT, fields, rest)
    if len(values) != int(length):
        raise ValueError(
            f"line {number}: {key} names {length} values and has {len(values)}"
        )
    into[key] = values


def read_plain(lines, indent, rest):
    """The value of a key without a length: a scalar or an object below it."""
    if rest is not None:
        return scalar(rest)
    if lines.indent() <= indent:
        return {}
    return read_object(lines, indent + INDENT, {})


def read_list(lines, indent, fields, rest):
    """A list: inline values, the rows of a table, or `- ` items."""
    if rest is not None:
        return cells(rest)
    if fields is not None:
        names = cells(fields)
        return [dict(zip(names, cells(row))) for row in rows_at(lines, indent)]
    items = []
    while lines.indent() == indent and lines.content().startswith("-"):
        _, content, number = lines.take()
        items.append(read_item(lines, indent + INDENT, content[1:].strip(), number))
    return items


def rows_at(lines, indent):
    rows = []
    while lines.indent() == indent:
        rows.append(lines.take()[1])
    return rows


def read_item(lines, indent, content, number):
    """One list item: an object when its first line is a `key:` line."""
    if not KEY_LINE.fullmatch(content):
        return scalar(content)
    item = {}
    read_field(lines, indent, content, number, item)
    return read_object(lines, indent, item)


def facts(journal):
    """What an expectation is compared with."""

    def met(block):
        return block.get("met") if isinstance(block, dict) else None

    objectives = journal.get("objectives") or {}
    verdicts = objectives.get("verdicts") or []
    return {
        "status": journal.get("status"),
        "hypothesis_before": met(journal.get("steady_state_before")),
        "hypothesis_after": met(journal.get("steady_state_after")),
        "rollbacks": len(journal.get("rollback_results") or []),
        "rollback_failures": journal.get("rollback_failures"),
        "load_result": journal.get("load_result") is not None,
        "objectives": {row["nfr_id"]: row["state"] for row in verdicts},
        "rpo_measured_s": objectives.get("rpo_measured_s"),
    }


def shown(value):
    return value if isinstance(value, str) else json.dumps(value)


def rpo_difference(wanted, measured_s):
    """The line for an RPO that is not in the range the entry names."""
    low, high = wanted["from"], wanted["to"]
    number = isinstance(measured_s, (int, float)) and not isinstance(measured_s, bool)
    if number and low <= measured_s <= high:
        return []
    seen = "none" if measured_s is None else shown(measured_s)
    return [f"rpo_measured_s: expected {low} to {high}, journal has {seen}"]


def load_difference(wanted, has):
    """The line for a load result that is wanted and not there, or the reverse."""
    if wanted == has:
        return []
    expected, got = ("one", "none") if wanted else ("none", "one")
    return [f"load_result: expected {expected}, journal has {got}"]


def measured(value):
    """True for a number above zero: a load that measured nothing reports 0."""
    return isinstance(value, (int, float)) and value is not True and value > 0


def load_problems(load, planned_s):
    """One line per thing a load result must show and does not.

    A load that was ended early, or whose numbers were never read, still
    leaves a load result. So the result must hold requests and a p99, and
    the load must have run the seconds its definition plans.
    """
    if load is None:
        return []
    found = [
        f"load_result: {name} is {shown(load.get(name))}, expected more than 0"
        for name in MEASURED
        if not measured(load.get(name))
    ]
    ran = load.get("duration_s")
    if planned_s is not None and not (measured(ran) and ran >= planned_s):
        found.append(
            f"load_result: the load ran {shown(ran)}s of a plan of {planned_s}s"
        )
    return found


def planned_load_s(definition):
    """The seconds the load of a definition is planned to run, if it says."""
    if not definition.is_file():
        return None
    load = read_toon(definition.read_text()).get("load")
    return load.get("duration_s") if isinstance(load, dict) else None


def differences(expect, seen):
    """One line per fact of the journal that is not what was expected."""
    missing = [name for name in FACTS if name not in expect]
    if missing:
        return [f"expectation has no {', '.join(missing)}"]
    expect = {"rollback_failures": 0, **expect}
    found = [
        f"{name}: expected {shown(expect[name])}, journal has {shown(seen[name])}"
        for name in COMPARED
        if expect[name] != seen[name]
    ]
    if "load_result" in expect:
        found += load_difference(expect["load_result"], seen["load_result"])
    if "rpo_measured_s" in expect:
        found += rpo_difference(expect["rpo_measured_s"], seen["rpo_measured_s"])
    wanted, got = expect["objectives"], seen["objectives"]
    for name in sorted(set(wanted) | set(got)):
        if wanted.get(name) != got.get(name):
            found.append(
                f"objective {name}: expected {wanted.get(name, 'none')}, "
                f"journal has {got.get(name, 'none')}"
            )
    return found


def result(name, found):
    return ("FAIL" if found else "ok", name, found)


def read_journal(path):
    """The journal at `path`, or the reason it cannot be used."""
    if not path.is_file():
        return None, [f"no journal {path.name}"]
    try:
        return read_toon(path.read_text()), []
    except (OSError, UnicodeError, ValueError) as error:
        return None, [f"{path.name}: {error}"]


def journal_name(name):
    return f"{name.removesuffix('.toon')}.journal.toon"


def check_experiment(name, entry, definition, journals):
    journal, problem = read_journal(journals / journal_name(name))
    if journal is None:
        return result(name, problem)
    expect = entry.get("expect", {})
    found = differences(expect, facts(journal))
    if expect.get("load_result"):
        found += load_problems(journal.get("load_result"), planned_load_s(definition))
    return result(name, found)


def check_gameday(name, entry, experiments, definition, journals):
    """The GameDay line, then one line per experiment in the order it ran."""
    journal, problem = read_journal(journals / journal_name(name))
    if journal is None:
        return [result(name, problem)]
    paths = [step["path"] for step in read_toon(definition.read_text())["experiments"]]
    records = journal.get("experiment_journals") or []
    found = gameday_differences(entry, journal, len(records), len(paths))
    if entry.get("expect", {}).get("load_result"):
        found += load_problems(journal.get("load_result"), planned_load_s(definition))
    lines = [result(name, found)]
    for index, path in enumerate(paths):
        found = record_differences(name, experiments.get(path, {}), records, index)
        lines.append(result(f"{name}: {path}", found))
    return lines


def gameday_differences(entry, journal, records, experiments):
    wants_load = bool(entry.get("expect", {}).get("load_result"))
    found = load_difference(wants_load, journal.get("load_result") is not None)
    if records > experiments:
        found.append(f"{records} records, the GameDay has {experiments} experiments")
    return found + score_differences(entry.get("expect", {}).get("score"), journal)


def two_decimals(value):
    """A number rounded to two decimals; anything else as it is."""
    if isinstance(value, (int, float)) and value is not True and value is not False:
        return round(value, 2)
    return value


def score_differences(wanted, journal):
    """One line per part of the GameDay score that is not the one expected.

    A recovery that was not measured has no key in the journal: null.
    """
    if wanted is None:
        return []
    missing = [name for name in SCORE if name not in wanted]
    if missing:
        return [f"score expectation has no {', '.join(missing)}"]
    score = journal.get("resilience_score") or {}
    seen = {
        "status": journal.get("compliance_status"),
        "recovery_compliance": two_decimals(score.get("recovery_compliance")),
    }
    return [
        f"score {name}: expected {shown(wanted[name])}, journal has {shown(seen[name])}"
        for name in SCORE
        if two_decimals(wanted[name]) != seen[name]
    ]


def record_differences(gameday, entry, records, index):
    if entry.get("within") != gameday:
        return [f"no expectation within {gameday}"]
    if index >= len(records):
        return [f"no record {index + 1} in the journal"]
    return differences(entry.get("expect", {}), facts(records[index]))


def standalone(proof, entry):
    """True when `proof` runs the experiment on its own, not within a GameDay."""
    return entry.get("proof") == proof and "within" not in entry


def listed(proof, expectations):
    """The experiments `proof` runs on their own, in the order of the file."""
    experiments = json.loads(Path(expectations).read_text()).get("experiments", {})
    return [name for name, entry in experiments.items() if standalone(proof, entry)]


def print_list(names):
    if not names:
        print("nothing to list: no entry names this proof", file=sys.stderr)
        return 1
    print("\n".join(names))
    return 0


def check_set(proof, expectations, journals):
    """Check every entry of `expectations` that `proof` runs."""
    body = json.loads(Path(expectations).read_text())
    experiments = body.get("experiments", {})
    journals = Path(journals)
    definitions = Path(expectations)
    lines = [
        check_experiment(name, entry, definitions.with_name(name), journals)
        for name, entry in experiments.items()
        if standalone(proof, entry)
    ]
    for name, entry in body.get("gamedays", {}).items():
        if entry.get("proof") == proof:
            definition = definitions.with_name(name)
            lines.extend(check_gameday(name, entry, experiments, definition, journals))
    return lines


def report(lines):
    """Print the lines; nonzero when one failed or there was nothing to check."""
    if not lines:
        print("nothing to check: no entry names this proof", file=sys.stderr)
        return 1
    for state, name, found in lines:
        print(f"{state:<4} {name}")
        for line in found:
            print(f"     {line}")
    return int(any(state == "FAIL" for state, _, _ in lines))


def main(arguments):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--proof", help="the proof whose entries are checked")
    parser.add_argument(
        "--describe", metavar="JOURNAL", help="print the facts of a journal"
    )
    parser.add_argument("--list", action="store_true", help="print what the proof runs")
    parser.add_argument("paths", nargs="*", metavar="EXPECTATIONS JOURNAL_DIR")
    options = parser.parse_args(arguments)
    if options.describe:
        print(
            json.dumps(facts(read_toon(Path(options.describe).read_text())), indent=2)
        )
        return 0
    if options.proof and options.list and len(options.paths) == 1:
        return print_list(listed(options.proof, options.paths[0]))
    if not options.proof or options.list or len(options.paths) != 2:
        parser.error(
            "give --proof NAME EXPECTATIONS JOURNAL_DIR, "
            "--proof NAME --list EXPECTATIONS, or --describe JOURNAL"
        )
    return report(check_set(options.proof, *options.paths))


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
