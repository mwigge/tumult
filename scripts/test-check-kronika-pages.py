#!/usr/bin/env python3
"""Tests for scripts/check-kronika-pages.py. Each test writes a small web UI
and its reference page in a temp dir, then breaks one of them."""

import importlib.util
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).with_name("check-kronika-pages.py")
_spec = importlib.util.spec_from_file_location("check_kronika_pages", SCRIPT)
kronika = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(kronika)

API = """\
async function get<T>(path: string): Promise<T> {
    const resp = await fetch(path);
    return (await resp.json()) as T;
}

export const api = {
    me: () => get<Me>('/api/me'),

    logout: () => send<Ok>('POST', '/api/auth/logout', {}),

    overview: (range: string) => get<Overview>(`/api/overview?range=${range}`),

    run: (id: string) => get<Run>(`/api/runs/${encodeURIComponent(id)}`),

    stopRun: (id: string) =>
        send<Ok>('POST', `/api/runs/${encodeURIComponent(id)}/stop`, {}),

    scoreTree: () => get<Tree>('/api/scores/tree'),

    dora: (lei = '') => get<Dora>(`/api/dora${lei ? `?lei=${lei}` : ''}`),

    ask: async (question: string) => {
        const resp = await fetch('/api/ask', {
            method: 'POST',
            body: JSON.stringify({ question }),
        });
        return resp.json();
    },
};
"""

TONE = """\
export const RUN_TONE: Readonly<Record<string, Tone>> = {
    completed: 'ok',
    deviated: 'fail',
    failed: 'fail',
    halted: 'fail',
    aborted: 'neutral',
    interrupted: 'neutral',
};
"""

COLOURS = """\
| Colour | Run ended as | What it means |
|---|---|---|
| Green | `completed` | A pass. |
| Red | `deviated`, `failed`, `halted` | A fail. |
| Grey | `aborted`, `interrupted` | No evidence. |
"""

PAGE = f"""\
# Krönika pages

Every page also calls `GET /api/me`, and `POST /api/auth/logout` when you
sign out.

## Colours of a run

{COLOURS}
## Evidence

### Overview

Route `/`. Cards and charts.

- **API:** `GET /api/overview`.

### Scores

Route `/scores`. The score tree.

- **API:** `GET /api/scores/tree`.

### DORA

Route `/scores?view=dora`. The DORA heatmap.

- **API:** `GET /api/dora`.

### Run detail

Route `/runs/{{id}}`. The run and its stop button.

- **API:** `GET /api/runs/{{id}}`,
  `POST /api/runs/{{id}}/stop`.

### Ask

Route `/ask`; old `/question` links open it. Ask a question.

- **API:** `POST /api/ask`.
"""


class RepoTest(unittest.TestCase):
    def setUp(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.write("web/src/lib/api.ts", API)
        self.write("web/src/lib/tone.ts", TONE)
        self.write(
            "web/src/routes/+layout.svelte",
            "<script>\n    import { api } from '#lib/api.js';\n"
            "    api.me();\n    const out = () => api.logout();\n</script>\n",
        )
        self.write(
            "web/src/routes/+page.svelte",
            "<script>\n    import { api } from '#lib/api.js';\n"
            "    api.overview('7d');\n</script>\n",
        )
        self.write(
            "web/src/routes/scores/+page.svelte",
            "<script>\n    import { api } from '$lib/api';\n"
            "    api.scoreTree();\n    api.dora();\n</script>\n",
        )
        self.write(
            "web/src/routes/runs/[id]/+page.svelte",
            "<script>\n    import { api } from '#lib/api.js';\n"
            "    import Stop from '#lib/components/Stop.svelte';\n"
            "    api.run(id);\n</script>\n",
        )
        self.write(
            "web/src/lib/components/Stop.svelte",
            "<script>\n    import { api } from '../api.js';\n"
            "    import { label } from './labels.js';\n"
            "    const stop = () => api.stopRun(id);\n</script>\n",
        )
        self.write("web/src/lib/components/labels.ts", "export const label = 'Stop';\n")
        self.write(
            "web/src/routes/ask/+page.svelte",
            "<script>\n    import { api } from '#lib/api.js';\n"
            "    api.ask(question);\n</script>\n",
        )
        self.write(
            "web/src/routes/ask/page.test.ts",
            "import { api } from '#lib/api.js';\napi.overview('1h');\n",
        )
        self.write("docs/reference/kronika-pages.md", PAGE)

    def write(self, path, text):
        file = self.root / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(text, encoding="utf-8")

    def edit(self, path, old, new):
        file = self.root / path
        text = file.read_text(encoding="utf-8")
        self.assertEqual(text.count(old), 1, old)
        file.write_text(text.replace(old, new), encoding="utf-8")

    def edit_page(self, old, new):
        self.edit("docs/reference/kronika-pages.md", old, new)

    def failures(self):
        return kronika.check(self.root)


class Calls(RepoTest):
    def test_a_page_that_lists_every_call_of_its_route_passes(self):
        self.assertEqual(self.failures(), [])

    def test_a_call_the_block_does_not_list_fails(self):
        self.edit_page("- **API:** `GET /api/overview`.", "- **API:** none.")
        self.assertEqual(
            self.failures(), [("Overview", "missing", "GET /api/overview")]
        )

    def test_a_listed_call_the_page_does_not_make_fails(self):
        self.edit_page("`POST /api/ask`.", "`POST /api/ask`, `GET /api/runs`.")
        self.assertEqual(self.failures(), [("Ask", "not-called", "GET /api/runs")])

    def test_a_call_in_an_imported_component_counts_for_the_page(self):
        self.edit_page(",\n  `POST /api/runs/{id}/stop`.", ".")
        self.assertEqual(
            self.failures(), [("Run detail", "missing", "POST /api/runs/{}/stop")]
        )

    def test_the_method_of_a_direct_fetch_is_read_from_its_options(self):
        self.edit_page("`POST /api/ask`.", "`GET /api/ask`.")
        self.assertEqual(
            self.failures(),
            [
                ("Ask", "missing", "POST /api/ask"),
                ("Ask", "not-called", "GET /api/ask"),
            ],
        )

    def test_two_blocks_on_one_route_folder_are_checked_as_one(self):
        self.edit_page("- **API:** `GET /api/dora`.", "- **API:** none.")
        self.assertEqual(
            self.failures(), [("Scores + DORA", "missing", "GET /api/dora")]
        )

    def test_a_test_file_in_a_route_folder_is_not_part_of_the_page(self):
        self.write(
            "web/src/routes/ask/+page.svelte",
            "<script>\n    import { api } from '#lib/api.js';\n"
            "    api.ask(question);\n    api.overview('1h');\n</script>\n",
        )
        self.assertEqual(self.failures(), [("Ask", "missing", "GET /api/overview")])

    def test_a_route_with_no_folder_fails(self):
        self.edit_page("Route `/ask`;", "Route `/question/ask`;")
        self.assertEqual(
            self.failures(),
            [
                ("Ask", "no-route", "/question/ask"),
                ("Ask", "not-called", "POST /api/ask"),
            ],
        )

    def test_a_call_of_the_layout_must_be_named_before_the_first_section(self):
        self.edit_page("Every page also calls `GET /api/me`, and", "Every page")
        self.assertEqual(self.failures(), [("intro", "layout-call", "GET /api/me")])

    def test_a_client_function_whose_endpoint_cannot_be_read_fails(self):
        self.edit(
            "web/src/lib/api.ts",
            "    me: () =>",
            "    health: () => get<Health>(HEALTH_PATH),\n\n    me: () =>",
        )
        self.assertEqual(self.failures(), [("api.ts", "no-endpoint", "health")])

    def test_a_page_with_no_page_blocks_fails(self):
        self.write("docs/reference/kronika-pages.md", PAGE.split("## Evidence")[0])
        self.assertEqual(
            self.failures(), [("pages", "no-blocks", "no `### Page` with a route")]
        )


class Colours(RepoTest):
    def test_a_run_ending_in_the_row_of_another_colour_fails(self):
        self.edit_page("`failed`, `halted` |", "`failed` |")
        self.edit_page(
            "`aborted`, `interrupted` |", "`aborted`, `interrupted`, `halted` |"
        )
        self.assertEqual(
            self.failures(),
            [
                (
                    "Colours of a run",
                    "colour",
                    "halted: Red in tone.ts, Grey on the page",
                )
            ],
        )

    def test_a_run_ending_missing_from_the_table_fails(self):
        self.edit_page("`aborted`, `interrupted` |", "`aborted` |")
        self.assertEqual(
            self.failures(),
            [
                (
                    "Colours of a run",
                    "colour",
                    "interrupted: Grey in tone.ts, not on the page",
                )
            ],
        )

    def test_a_status_that_is_not_a_run_ending_fails(self):
        self.edit_page(
            "`aborted`, `interrupted` |", "`aborted`, `interrupted`, `queued` |"
        )
        self.assertEqual(
            self.failures(),
            [("Colours of a run", "colour", "queued: on the page, not in tone.ts")],
        )

    def test_a_status_in_two_rows_fails(self):
        self.edit_page("| Green | `completed` |", "| Green | `completed`, `halted` |")
        self.assertEqual(
            self.failures(),
            [("Colours of a run", "colour", "halted: in more than one row")],
        )

    def test_a_page_with_no_colour_table_fails(self):
        self.edit_page(COLOURS, "")
        self.assertEqual(
            self.failures(),
            [("Colours of a run", "colour", "no row of the table names a run ending")],
        )


class Command(RepoTest):
    def run_script(self):
        return subprocess.run(
            [sys.executable, str(SCRIPT), "--root", str(self.root)],
            capture_output=True,
            text=True,
        )

    def test_the_command_passes_quietly_on_a_page_that_agrees(self):
        result = self.run_script()
        self.assertEqual((result.returncode, result.stdout), (0, ""))

    def test_the_command_names_each_failure_and_exits_one(self):
        self.edit_page("- **API:** `GET /api/overview`.", "- **API:** none.")
        result = self.run_script()
        self.assertEqual(result.returncode, 1)
        self.assertIn("Overview: missing: GET /api/overview", result.stdout)


if __name__ == "__main__":
    unittest.main()
