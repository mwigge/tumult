import assert from "node:assert/strict";
import { Buffer } from "node:buffer";
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { test } from "node:test";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const playwright = process.env.PLAYWRIGHT_MODULE || "@playwright/test";
const { chromium } = await import(
  playwright.startsWith("/") ? pathToFileURL(playwright) : playwright
);

test("learner can safely run a lab, review evidence, and configure a read-only tutor", async () => {
  const calls = [];
  let history = [];
  const run = {
    id: "run-1",
    scenario_id: "latency",
    title: "Slow dependency",
    status: "running",
    phase: "during",
    events: [
      { at: new Date().toISOString(), message: "Fault applied to sandbox" },
    ],
    baseline: {
      requests: 5,
      successes: 5,
      errors: 0,
      p95_ms: 12,
      error_rate: 0,
    },
    during: {
      requests: 5,
      successes: 5,
      errors: 0,
      p95_ms: 512,
      error_rate: 0,
    },
    recovery: null,
    journal_available: false,
  };
  const status = {
    engine: { available: true, version: "2.22.0" },
    target: { healthy: true },
    provider: { configured: false },
    mode: "local-lab",
    active_run_id: null,
  };
  const server = createServer(async (req, res) => {
    try {
      const chunks = [];
      for await (const part of req) chunks.push(part);
      const body = chunks.length
        ? JSON.parse(Buffer.concat(chunks))
        : undefined;
      calls.push({ url: req.url, method: req.method, body });
      let value;
      if (req.url === "/api/status") value = status;
      else if (req.url === "/api/scenarios")
        value = [
          {
            id: "latency",
            title: "Slow dependency",
            description: "Observe a slow dependency.",
            hypothesis: "The API stays within 200 ms.",
            learning: ["Separate latency from availability."],
            fault: "delay",
            target: "sandbox API",
            duration_s: 10,
            threshold_ms: 200,
          },
        ];
      else if (req.url === "/api/runs" && req.method === "GET") value = history;
      else if (req.url === "/api/runs" || req.url === "/api/runs/run-1")
        value = run;
      else if (req.url === "/api/runs/run-1/stop") {
        run.status = "stopped";
        run.phase = "complete";
        run.journal_available = true;
        run.recovery = {
          requests: 5,
          successes: 5,
          errors: 0,
          p95_ms: 13,
          error_rate: 0,
        };
        value = run;
      } else if (req.url === "/api/settings")
        value = {
          configured: req.method !== "DELETE",
          provider: body?.provider || "openai",
          model: body?.model || "test-model",
        };
      else if (req.url === "/api/ask")
        value = {
          answer:
            '<img src=x onerror="window.pwned=true">\nLatency increased; inspect recovery.',
          provider: "openai",
          model: "test-model",
        };
      else {
        const filename = req.url === "/" ? "index.html" : req.url.slice(1);
        if (
          !["index.html", "guide.html", "app.js", "styles.css"].includes(
            filename,
          )
        ) {
          res.writeHead(404).end();
          return;
        }
        res.setHeader(
          "Content-Type",
          {
            "index.html": "text/html",
            "guide.html": "text/html",
            "app.js": "text/javascript",
            "styles.css": "text/css",
          }[filename],
        );
        res.end(await readFile(resolve(root, "web", filename)));
        return;
      }
      res.setHeader("Content-Type", "application/json");
      res.end(JSON.stringify(value));
    } catch (error) {
      res.writeHead(500).end(String(error));
    }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  let browser;
  try {
    browser = await chromium.launch({
      headless: true,
      ...(process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE
        ? { executablePath: process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE }
        : {}),
    });
    const page = await browser.newPage();
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.goto(`http://127.0.0.1:${server.address().port}`);
    await page.getByRole("button", { name: /Slow dependency/ }).click();
    assert.equal(
      await page
        .getByRole("button", { name: "Run experiment", exact: true })
        .isDisabled(),
      true,
    );
    await page.getByLabel(/I understand this changes only/).check();
    await page
      .getByRole("button", { name: "Run experiment", exact: true })
      .click();
    await page
      .getByRole("button", { name: "Stop experiment", exact: true })
      .waitFor();
    assert.deepEqual(
      calls.find((c) => c.method === "POST" && c.url === "/api/runs").body,
      { scenario_id: "latency", armed: true },
    );
    await page
      .getByRole("button", { name: "Stop experiment", exact: true })
      .click();
    await page.getByRole("link", { name: "Export evidence" }).waitFor();
    assert.equal(
      await page
        .getByRole("link", { name: "Export evidence" })
        .getAttribute("href"),
      "/api/runs/run-1/export",
    );
    await page.getByRole("button", { name: "AI settings" }).click();
    await page.getByLabel("Model", { exact: true }).fill("test-model");
    await page
      .getByLabel("API key", { exact: true })
      .fill("never-store-this-key");
    await page.getByRole("button", { name: "Save connection" }).click();
    await page.getByRole("dialog").waitFor({ state: "hidden" });
    await page.getByRole("button", { name: "AI settings" }).click();
    assert.equal(
      await page.getByLabel("API key", { exact: true }).inputValue(),
      "",
    );
    await page.getByRole("button", { name: "Close settings" }).click();
    await page.getByLabel("Ask your tutor").fill("Explain the results");
    await page.getByRole("button", { name: "Ask tutor", exact: true }).click();
    await page.getByText(/Latency increased; inspect recovery/).waitFor();
    assert.equal(await page.evaluate(() => window.pwned), undefined);
    assert.equal(await page.locator("#tutor-answer img").count(), 0);
    assert.deepEqual(
      await page.evaluate(() => ({
        local: Object.keys(window.localStorage),
        session: Object.keys(window.sessionStorage),
      })),
      { local: [], session: [] },
    );
    for (const width of [320, 768, 1024, 1440]) {
      await page.setViewportSize({ width, height: 900 });
      assert.equal(
        await page.evaluate(
          () => document.documentElement.scrollWidth <= window.innerWidth,
        ),
        true,
        `overflow at ${width}px`,
      );
    }
    run.status = "interrupted";
    history = [run];
    await page.getByRole("button", { name: "Refresh", exact: true }).click();
    await page.locator(".history-item").click();
    await page
      .locator("#run-status")
      .filter({ hasText: "interrupted" })
      .waitFor();
    assert.equal(
      await page
        .getByRole("button", { name: "Stop experiment", exact: true })
        .isVisible(),
      false,
    );
    if (process.env.UI_SCREENSHOT)
      await page.screenshot({
        path: process.env.UI_SCREENSHOT,
        fullPage: true,
      });
    assert.equal(
      await page
        .getByRole("link", { name: "Learning guide", exact: true })
        .count(),
      1,
    );
    await page
      .getByRole("link", { name: "Learning guide", exact: true })
      .click();
    await page
      .getByRole("heading", { name: "Turn a fault into understanding." })
      .waitFor();
    for (const width of [320, 768, 1024, 1440]) {
      await page.setViewportSize({ width, height: 900 });
      assert.equal(
        await page.evaluate(
          () => document.documentElement.scrollWidth <= window.innerWidth,
        ),
        true,
        `guide overflow at ${width}px`,
      );
    }
    await page
      .getByRole("link", { name: "Back to the lab", exact: true })
      .click();
    await page.getByRole("heading", { name: "The experiment bench" }).waitFor();
    assert.deepEqual(errors, []);
  } finally {
    await browser?.close();
    await new Promise((resolve) => server.close(resolve));
  }
});
