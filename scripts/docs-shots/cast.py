#!/usr/bin/env python3
"""Record the terminal demo of tumult.rs from a real session.

shoot.sh runs this after data.py, against the same docs daemon. Every
command below runs for real with the tumult CLI of the work dir, and every
line of output in the recording is what the command printed. Only the
typing and the pauses are paced by this script. JSON log lines on stderr
are left out.

The commands run in the work dir with relative paths, so no path of the
machine shows in the recording.

Writes an asciinema v2 file, the format the player of the site reads.

Usage: cast.py <work dir> <out.cast>
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
import time
from pathlib import Path

from data import EXPERIMENTS, PROOF, Stack, wait_ready

WIDTH, HEIGHT = 110, 35
COPPER = "\x1b[38;5;173m"
DIM = "\x1b[2m"
BOLD = "\x1b[1m"
GREEN = "\x1b[1;32m"
RESET = "\x1b[0m"
# The checkout build the session tests: 3 ms, and every 6th request 60 ms.
BUILD = ("3", "6", "60")


class Cast:
    """The events of one recording, with a clock of their own."""

    def __init__(self, stack: Stack):
        self.stack = stack
        self.events: list[list] = []
        self.clock = 0.6

    def emit(self, text: str, pause: float = 0.0) -> None:
        self.clock += pause
        self.events.append([round(self.clock, 3), "o", text])

    def section(self, title: str) -> None:
        rule = "─" * max(0, WIDTH - len(title) - 6)
        self.emit("\r\n", 0.8)
        self.emit(f"{COPPER}{BOLD}── {title} {RESET}{DIM}{rule}{RESET}\r\n", 0.05)
        self.emit("", 0.7)

    def comment(self, text: str) -> None:
        self.emit(f"{DIM}# {text}{RESET}\r\n", 0.5)

    def type_command(self, command: str) -> None:
        self.emit(f"{GREEN}❯{RESET} ", 0.4)
        for char in command:
            self.emit(char, 0.03 if char == " " else 0.018)
        self.emit("\r\n", 0.35)

    def line(self, text: str) -> None:
        """One line of output, wrapped at a space to the width of the cast."""
        while len(text) > WIDTH - 2:
            cut = text.rfind(" ", 0, WIDTH - 2)
            cut = cut if cut >= 40 else WIDTH - 2
            self.emit(text[:cut] + "\r\n", 0.02)
            text = "    " + text[cut:].lstrip()
        self.emit(text + "\r\n", 0.028)

    def run(self, *args: str, keep: str = "", pause: float = 1.6) -> None:
        """Type `tumult <args>`, run it in the work dir and show its output."""
        self.type_command(" ".join(("tumult", *args)))
        done = subprocess.run(
            [self.stack.tumult, *args],
            cwd=self.stack.work,
            env=self.stack.cli_env(),
            capture_output=True,
            text=True,
            check=False,
        )
        lines = (done.stdout + done.stderr).splitlines()
        lines = [line for line in lines if line.strip() and not line.startswith("{")]
        if keep:
            lines = [line for line in lines if line.startswith(tuple(keep.split("|")))]
        if not lines:
            raise SystemExit(f"tumult {' '.join(args)} printed nothing to show")
        for line in lines:
            self.line(line)
        self.emit("", pause)

    def write(self, path: Path) -> None:
        header = {
            "version": 2,
            "width": WIDTH,
            "height": HEIGHT,
            "timestamp": int(time.time()),
            "env": {"SHELL": "/bin/bash", "TERM": "xterm-256color"},
        }
        lines = [json.dumps(header)] + [json.dumps(event) for event in self.events]
        path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def session(cast: Cast, port: int) -> None:
    cast.section("tumult 3.0 · run · measure · judge · report")
    cast.run("--version", pause=0.8)

    cast.section("1 · run a soak against the new checkout build")
    cast.comment("24 seconds of load on checkout 1.3.0; the journal goes to the daemon")
    cast.run(
        "run",
        "checkout-soak.toon",
        "--var",
        f"port={port}",
        "--journal-path",
        "checkout-soak.journal.toon",
        "--force",
        keep="Running|Status|Hypothesis|Objectives|Load|Ingested",
    )

    cast.section("2 · did the new build get slower?")
    cast.comment("the daemon compares the two latest versions of checkout")
    cast.run("perf", "checkout", "--env", "demo", pause=2.6)

    cast.section("3 · may the message queue ship?")
    cast.run("gate", "message-queue", pause=2.2)

    cast.section("4 · one file for the reviewer")
    cast.run("report", "checkout-soak.journal.toon", "--output", "checkout-soak.html")

    cast.emit("\r\n", 1.0)
    cast.emit(
        f"{COPPER}{BOLD}every verdict comes from stored evidence: tumult.rs{RESET}\r\n",
        0.1,
    )
    cast.emit("", 2.5)


def main() -> int:
    if len(sys.argv) != 3:
        sys.stderr.write(__doc__)
        return 2
    stack = Stack(Path(sys.argv[1]))
    out = Path(sys.argv[2])
    shutil.copy(EXPERIMENTS / "checkout-soak.toon", stack.work / "checkout-soak.toon")
    port = stack.target_port
    with (stack.work / "cast-service.log").open("w") as service_log:
        service = subprocess.Popen(
            [sys.executable, str(PROOF / "target.py"), str(port), *BUILD],
            stdout=service_log,
            stderr=subprocess.STDOUT,
        )
    try:
        wait_ready(port)
        cast = Cast(stack)
        session(cast, port)
    finally:
        service.terminate()
        service.wait(timeout=10)
    cast.write(out)
    sys.stdout.write(
        f"   cast: {out.name}, {len(cast.events)} events, {cast.clock:.0f} s\n"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
