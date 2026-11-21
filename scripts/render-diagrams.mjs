#!/usr/bin/env node
// Draw the docs diagrams from their sources. See
// scripts/diagrams/README.md and .github/workflows/site-preview.yml.
//
// A diagram is an archify JSON source, <name>.json, in the diagram dir. For
// each source this writes <name>.svg next to it: archify renders the source
// into its HTML viewer, and Chromium exports the light SVG through the
// viewer's own Export menu, with the settings of the 3.0 pictures (a 1600 by
// 1000 viewport at scale 2, light theme). archify checks every `sources` path
// of a diagram at the revision the diagram pins, in this git checkout.
//
// --check writes nothing into the diagram dir. It draws into a temp dir and
// fails, with the file and the reason, on:
//   render-failed  a source that archify refuses
//   differs        a picture that is not what its source draws now
//   missing        a source with no picture
//   no-source      a picture that no source draws: a .png, or an .svg with no
//                  .json
//
// archify and Playwright are pinned in scripts/diagrams/package.json: run
// `npm ci` and `npx playwright install chromium` there first.
//
// Usage: node scripts/render-diagrams.mjs [--check] [--dir DIR] [name ...]

import { spawnSync } from 'node:child_process';
import { existsSync, mkdtempSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, dirname, extname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const TOOLS = join(ROOT, 'scripts', 'diagrams', 'node_modules');
const ARCHIFY = join(TOOLS, 'archify', 'archify', 'bin', 'archify.mjs');
const PLAYWRIGHT = join(TOOLS, 'playwright', 'index.mjs');
const DIAGRAMS = join(ROOT, 'docs', 'images', '3.0', 'diagrams');
const PICTURES = new Set(['.svg', '.png']);
const VIEW = {
  viewport: { width: 1600, height: 1000 },
  deviceScaleFactor: 2,
  colorScheme: 'light',
  acceptDownloads: true,
};
const READY_MS = 30000;

// The sources of the dir, or of the names asked for: name to source path.
function sources(dir, names) {
  const all = new Map(
    readdirSync(dir)
      .filter((file) => extname(file) === '.json')
      .sort()
      .map((file) => [basename(file, '.json'), join(dir, file)]),
  );
  if (!names.length) return { found: all, unknown: [] };
  const unknown = names.filter((name) => !all.has(name));
  return { found: new Map(names.map((name) => [name, all.get(name)])), unknown };
}

// archify renders one source into its HTML viewer, or says why it cannot.
function renderHtml(source, html) {
  const { diagram_type: type } = JSON.parse(readFileSync(source, 'utf8'));
  const result = spawnSync(
    process.execPath,
    [ARCHIFY, 'render', type, source, html, '--quality', 'showcase', '--repo-root', ROOT],
    { encoding: 'utf8', env: { ...process.env, ARCHIFY_UPDATE_CHECK_DISABLED: '1' } },
  );
  if (result.status === 0) return null;
  const reason = `${result.stderr}${result.stdout}`.trim().split('\n').pop();
  return reason || `archify exited with ${result.status ?? result.signal}`;
}

// The light SVG of one HTML viewer, through the viewer's own Export menu.
async function exportSvg(browser, html, out) {
  const context = await browser.newContext(VIEW);
  try {
    await context.addInitScript(() => localStorage.setItem('archify-theme', 'light'));
    const page = await context.newPage();
    await page.goto(pathToFileURL(html).href);
    await page.waitForFunction(
      () => window.Archify?.exportMenu && document.fonts.status === 'loaded',
      null,
      { timeout: READY_MS },
    );
    const [download] = await Promise.all([
      page.waitForEvent('download', { timeout: READY_MS }),
      page.evaluate(() => window.Archify.exportMenu.run('svg-light')),
    ]);
    await download.saveAs(out);
  } finally {
    await context.close();
  }
}

// Draw every source into `out`; the failures as `file: rule: reason`.
async function drawAll(found, work, out) {
  const { chromium } = await import(pathToFileURL(PLAYWRIGHT).href);
  const browser = await chromium.launch();
  const failures = [];
  try {
    for (const [name, source] of found) {
      const html = join(work, `${name}.html`);
      const refused = renderHtml(source, html);
      if (refused) {
        failures.push(`${name}.json: render-failed: ${refused}`);
        continue;
      }
      await exportSvg(browser, html, join(out, `${name}.svg`));
      console.log(`drew ${name}.svg`);
    }
  } finally {
    await browser.close();
  }
  return failures;
}

// What the check finds when it holds the drawn pictures against the dir.
function compare(dir, found, drawn, everyName) {
  const failures = [];
  for (const name of found.keys()) {
    const fresh = join(drawn, `${name}.svg`);
    const kept = join(dir, `${name}.svg`);
    if (!existsSync(fresh)) continue;
    if (!existsSync(kept)) {
      failures.push(`${name}.svg: missing: run node scripts/render-diagrams.mjs ${name}`);
    } else if (!readFileSync(fresh).equals(readFileSync(kept))) {
      failures.push(`${name}.svg: differs: run node scripts/render-diagrams.mjs ${name}`);
    }
  }
  if (!everyName) return failures;
  for (const file of readdirSync(dir).sort()) {
    if (!PICTURES.has(extname(file))) continue;
    if (extname(file) === '.png') {
      failures.push(`${file}: no-source: the command draws .svg only; delete it`);
    } else if (!found.has(basename(file, '.svg'))) {
      failures.push(`${file}: no-source: delete it, or add its source`);
    }
  }
  return failures;
}

async function main(argv) {
  const { values, positionals } = parseArgs({
    args: argv,
    allowPositionals: true,
    options: { check: { type: 'boolean' }, dir: { type: 'string' } },
  });
  const dir = resolve(values.dir ?? DIAGRAMS);
  if (!existsSync(ARCHIFY) || !existsSync(PLAYWRIGHT)) {
    console.error(
      'render-diagrams: the pinned tools are missing; run `npm ci` and ' +
        '`npx playwright install chromium` in scripts/diagrams',
    );
    return 2;
  }
  if (!existsSync(dir)) {
    console.error(`render-diagrams: not a dir: ${dir}`);
    return 2;
  }
  const { found, unknown } = sources(dir, positionals);
  for (const name of unknown) {
    console.error(`render-diagrams: ${name}: no source ${name}.json in ${dir}`);
  }
  if (unknown.length) return 2;
  const work = mkdtempSync(join(tmpdir(), 'render-diagrams-'));
  try {
    const out = values.check ? work : dir;
    const failures = await drawAll(found, work, out);
    if (values.check) failures.push(...compare(dir, found, work, !positionals.length));
    for (const line of failures) console.log(line);
    if (failures.length) {
      console.log(`${failures.length} diagram failure(s).`);
      return 1;
    }
    const verb = values.check ? 'match their sources' : `drawn in ${dir}`;
    console.log(`${found.size} diagram(s) ${verb}.`);
    return 0;
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

main(process.argv.slice(2)).then(
  (code) => {
    process.exitCode = code;
  },
  (error) => {
    console.error(error);
    process.exitCode = 1;
  },
);
