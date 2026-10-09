#!/usr/bin/env python3
"""Gate canonical, CLI-generated TOON journals on experiment outcomes.

This checks the generated journal layout, not arbitrary user TOON. Status in
probe output or nested activities cannot satisfy the experiment-level gate.
"""

import re
import sys
from pathlib import Path


def completed_record(lines: list[str], indent: str = "") -> bool:
    """Require one successful status and an explicit zero rollback-failure count."""
    statuses = [line for line in lines if line.startswith(f"{indent}status:")]
    rollbacks = [
        line for line in lines if line.startswith(f"{indent}rollback_failures:")
    ]
    return statuses == [f"{indent}status: completed"] and rollbacks == [
        f"{indent}rollback_failures: 0"
    ]


def is_completed(text: str) -> bool:
    """Check a single experiment or every record in a nonempty GameDay."""
    lines = text.splitlines()
    if not any(line.startswith("gameday_id:") for line in lines):
        return completed_record(lines)
    headers = [
        (i, re.fullmatch(r"experiment_journals\[(\d+)\]:", line))
        for i, line in enumerate(lines)
        if line.startswith("experiment_journals[")
    ]
    if len(headers) != 1 or headers[0][1] is None:
        return False
    start, header = headers[0]
    expected = int(header[1])
    records: list[list[str]] = []
    for line in lines[start + 1 :]:
        if line and not line.startswith(" "):
            break
        if line.startswith("  - experiment_title:"):
            records.append([])
        elif records:
            records[-1].append(line)
    return (
        expected > 0
        and len(records) == expected
        and all(completed_record(record, "    ") for record in records)
    )


def main(paths: list[str]) -> int:
    """Return nonzero for missing, unreadable, incomplete or failed evidence."""
    if not paths:
        print("No journal paths supplied", file=sys.stderr)
        return 1
    failed = False
    for name in paths:
        try:
            complete = is_completed(Path(name).read_text())
        except (OSError, UnicodeError) as error:
            print(f"{name}: {error}", file=sys.stderr)
            complete = False
        print(f"{'ok' if complete else 'NOT-DONE'} {name}")
        failed |= not complete
    return int(failed)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
