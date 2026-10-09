"""Exercise the running Compose lab without an AI key or fabricated responses."""

import json
import os
from pathlib import Path

from playwright.sync_api import expect, sync_playwright

ROOT = Path(__file__).parents[1]
OUTPUT = ROOT / "test-results"


def main() -> None:
    OUTPUT.mkdir(exist_ok=True)
    with sync_playwright() as browser_api:
        options = {"headless": True}
        executable = os.environ.get("PLAYWRIGHT_CHROMIUM_EXECUTABLE")
        if executable:
            options["executable_path"] = executable
        browser = browser_api.chromium.launch(**options)
        try:
            page = browser.new_page(viewport={"width": 1440, "height": 1000})
            errors = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.goto(os.environ.get("LAB_URL", "http://127.0.0.1:8089"))
            page.wait_for_load_state("networkidle")
            expect(page.locator("#engine-status")).to_contain_text("ready")
            expect(page.locator("#target-status")).to_contain_text("responding")
            expect(page.locator(".scenario-card")).to_have_count(4)
            page.get_by_role("button", name="AI settings").click()
            expect(page.get_by_label("Model", exact=True)).to_have_value("gpt-4.1-mini")
            page.get_by_label("Provider", exact=True).select_option("anthropic")
            expect(page.get_by_label("Model", exact=True)).to_have_value(
                "claude-sonnet-4-6"
            )
            page.get_by_label("API key", exact=True).fill(
                "dummy-browser-secret-never-send"
            )
            page.get_by_role("button", name="Save connection").click()
            expect(page.get_by_role("dialog")).not_to_be_visible()
            page.get_by_role("button", name="AI settings").click()
            expect(page.get_by_label("API key", exact=True)).to_have_value("")
            page.locator("#settings-clear").click()
            expect(page.get_by_role("dialog")).not_to_be_visible()
            page.get_by_role("button", name="AI settings").click()
            page.get_by_label("Provider", exact=True).select_option("ollama")
            expect(page.get_by_label("Model", exact=True)).to_have_value("llama3.2:3b")
            page.get_by_role("button", name="Save connection").click()
            expect(page.get_by_role("dialog")).not_to_be_visible()
            page.get_by_role("button", name="AI settings").click()
            page.locator("#settings-clear").click()
            page.get_by_role(
                "button", name="EXPERIMENT 01 Slow responses", exact=False
            ).click()
            expect(
                page.get_by_role("button", name="Run experiment", exact=True)
            ).to_be_disabled()
            page.get_by_label("I understand this changes only", exact=False).check()
            page.get_by_role("button", name="Run experiment", exact=True).click()
            expect(page.locator("#run-status")).to_have_text("completed", timeout=65000)
            expect(page.locator("#verdict-title")).to_contain_text(
                "deviated", ignore_case=True
            )
            exports = {}
            for label in ("Export evidence", "Tumult journal", "Experiment definition"):
                with page.expect_download() as download:
                    page.get_by_role("link", name=label, exact=True).click()
                path = OUTPUT / download.value.suggested_filename
                download.value.save_as(path)
                exports[label] = path
            record = json.loads(exports["Export evidence"].read_text())["run"]
            assert record["baseline"]["healthy"]
            assert record["during"]["p95_ms"] >= 350
            assert record["during"]["errors"] == 0
            assert record["recovery"]["healthy"]
            assert record["native_status"] == "completed"
            assert (
                "steady_state_hypothesis:"
                in exports["Experiment definition"].read_text()
            )
            assert "status: completed" in exports["Tumult journal"].read_text()
            page.screenshot(path=str(OUTPUT / "lab-desktop.png"), full_page=True)
            for width in (320, 768, 1024):
                page.set_viewport_size({"width": width, "height": 900})
                assert page.evaluate(
                    "document.documentElement.scrollWidth <= innerWidth"
                )
            page.screenshot(path=str(OUTPUT / "lab-mobile.png"), full_page=True)
            page.set_viewport_size({"width": 1440, "height": 1000})
            page.get_by_role(
                "button", name="EXPERIMENT 04 Database timeout", exact=False
            ).click()
            page.get_by_label("I understand this changes only", exact=False).check()
            page.get_by_role("button", name="Run experiment", exact=True).click()
            expect(page.locator("#run-description")).to_contain_text(
                "during", timeout=15000
            )
            page.get_by_role("button", name="Stop experiment", exact=True).click()
            expect(page.locator("#run-status")).to_have_text("stopped", timeout=25000)
            with page.expect_download() as download:
                page.get_by_role("link", name="Export evidence", exact=True).click()
            stopped_file = OUTPUT / download.value.suggested_filename
            download.value.save_as(stopped_file)
            stopped = json.loads(stopped_file.read_text())["run"]
            assert stopped["recovery"]["healthy"]
            assert stopped["verdict"] == "inconclusive"
            page.reload()
            page.wait_for_load_state("networkidle")
            expect(page.locator("#provider-status")).to_contain_text("Optional")
            assert (
                page.evaluate(
                    "Object.keys(localStorage).length + Object.keys(sessionStorage).length"
                )
                == 0
            )
            assert not errors, errors
            (OUTPUT / "browser-result.json").write_text(
                json.dumps(
                    {
                        "completed_run": record["id"],
                        "stopped_run": stopped["id"],
                        "baseline_p95_ms": record["baseline"]["p95_ms"],
                        "during_p95_ms": record["during"]["p95_ms"],
                        "recovery_p95_ms": record["recovery"]["p95_ms"],
                        "browser_errors": errors,
                    },
                    indent=2,
                )
            )
        finally:
            browser.close()


if __name__ == "__main__":
    main()
