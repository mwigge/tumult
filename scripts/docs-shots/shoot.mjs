#!/usr/bin/env node
// Take the docs screenshots of Krönika, one picture per entry of shots.json.
// shoot.sh runs this after data.py. See README.md next to this file.
//
// Usage: node scripts/docs-shots/shoot.mjs <work dir> <out dir> [name ...]
//
// <work dir> holds stack.json (url and admin password of the docs daemon),
// ids.json (the ids data.py found) and report.html. With names, only those
// entries are taken.
//
// An entry of shots.json:
//   name    the file name, without .png
//   route   the page to open; {key} is replaced by the value of key in
//           ids.json
//   file    instead of route: a file of the work dir, opened without
//           signing in (the HTML report)
//   login   false: open the page signed out
//   height  the viewport height in CSS pixels (default 900; width is 1440)
//   steps   what to do before the picture, in order; each step is one of
//           click, fill [selector, text], scrollTo (with offset), blur,
//           reload, waitFor (a selector that must show) and wait (ms)
//
// Every picture is 1440 CSS pixels wide at device scale 2, in the light
// theme, en-GB and Europe/Stockholm. A PNG with any chunk other than IHDR,
// IDAT and IEND fails the run, so no picture carries metadata.

import { readFileSync, mkdirSync, statSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = resolve(HERE, '..', '..');
const PLAYWRIGHT = join(ROOT, 'web', 'node_modules', 'playwright', 'index.mjs');
const WIDTH = 1440;
const SETTLE_MS = 1800;
const PNG_CHUNKS = new Set(['IHDR', 'IDAT', 'IEND']);

const [work, out, ...only] = process.argv.slice(2);
if (!work || !out) {
  process.stderr.write('usage: node shoot.mjs <work dir> <out dir> [name ...]\n');
  process.exit(2);
}
const stack = JSON.parse(readFileSync(join(work, 'stack.json'), 'utf8'));
const ids = JSON.parse(readFileSync(join(work, 'ids.json'), 'utf8'));
const shots = JSON.parse(readFileSync(join(HERE, 'shots.json'), 'utf8'));
const unknown = only.filter((name) => !shots.some((shot) => shot.name === name));
if (unknown.length) throw new Error(`not in shots.json: ${unknown.join(', ')}`);

// `{key}` in a route or a step, from ids.json.
function fill(text) {
  return text.replace(/\{([a-z.]+)\}/g, (_, key) => {
    if (!(key in ids)) throw new Error(`no id ${key} in ids.json`);
    return encodeURIComponent(ids[key]);
  });
}

// The chunk types of a PNG file, in order.
function pngChunks(file) {
  const bytes = readFileSync(file);
  const types = [];
  for (let at = 8; at < bytes.length; ) {
    const length = bytes.readUInt32BE(at);
    types.push(bytes.toString('latin1', at + 4, at + 8));
    at += 12 + length;
  }
  return types;
}

async function settle(page) {
  await page.waitForLoadState('networkidle').catch(() => {});
  // The ECharts canvases draw after the data arrives.
  await page.waitForTimeout(SETTLE_MS);
  await page.waitForLoadState('networkidle').catch(() => {});
}

async function step(page, s) {
  if (s.click) await page.locator(fill(s.click)).first().click();
  if (s.fill) await page.locator(s.fill[0]).first().fill(fill(s.fill[1]));
  if (s.reload) await page.reload();
  if (s.waitFor) await page.locator(s.waitFor).first().waitFor({ timeout: 120000 });
  if (s.scrollTo) {
    await page
      .locator(s.scrollTo)
      .first()
      .evaluate(
        (el, offset) => window.scrollTo(0, el.getBoundingClientRect().top + window.scrollY - offset),
        s.offset ?? 16,
      );
  }
  if (s.blur) {
    await page.evaluate(() => {
      const active = document.activeElement;
      if (active && active !== document.body) active.blur();
    });
  }
  if (s.wait) await page.waitForTimeout(s.wait);
  await settle(page);
}

async function signIn(page) {
  await page.goto(`${stack.url}/login`);
  await page.fill('input[autocomplete=username]', 'admin');
  await page.fill('input[autocomplete=current-password]', stack.admin_password);
  await Promise.all([
    page.waitForURL((url) => !url.pathname.startsWith('/login')),
    page.click('button[type=submit]'),
  ]);
}

async function take(page, shot) {
  await page.setViewportSize({ width: WIDTH, height: shot.height ?? 900 });
  const url = shot.file
    ? pathToFileURL(resolve(work, shot.file)).href
    : `${stack.url}${fill(shot.route)}`;
  await page.goto(url);
  await settle(page);
  for (const s of shot.steps ?? []) await step(page, s);
  if (shot.login !== false && !shot.file && new URL(page.url()).pathname === '/login') {
    throw new Error(`${shot.name}: the page sent us to the login`);
  }
  const file = join(out, `${shot.name}.png`);
  await page.screenshot({ path: file, type: 'png' });
  const chunks = new Set(pngChunks(file));
  const extra = [...chunks].filter((type) => !PNG_CHUNKS.has(type));
  if (extra.length) throw new Error(`${shot.name}: the PNG holds ${extra.join(', ')}`);
  process.stdout.write(`   shot: ${shot.name} ${statSync(file).size} bytes\n`);
}

const { chromium } = await import(pathToFileURL(PLAYWRIGHT).href);
mkdirSync(out, { recursive: true });
const browser = await chromium.launch();
try {
  const context = () =>
    browser.newContext({
      viewport: { width: WIDTH, height: 900 },
      deviceScaleFactor: 2,
      colorScheme: 'light',
      locale: 'en-GB',
      timezoneId: 'Europe/Stockholm',
    });
  const outside = await context();
  const inside = await context();
  for (const ctx of [outside, inside]) {
    // The docs daemon and local files only: a page that asks for more
    // gets nothing, so a picture never depends on the network.
    await ctx.route('**/*', (route) => {
      const url = route.request().url();
      const local = url.startsWith(stack.url) || /^(file|data|blob):/.test(url);
      return local ? route.continue() : route.abort();
    });
    await ctx.addInitScript(() => {
      try {
        localStorage.setItem('tumult.theme', 'light');
      } catch {
        // file:// pages have no storage; the report follows colorScheme.
      }
    });
  }
  const signedOut = await outside.newPage();
  const page = await inside.newPage();
  for (const p of [signedOut, page]) {
    p.on('pageerror', (error) => process.stderr.write(`pageerror: ${error.message}\n`));
  }
  await signIn(page);
  for (const shot of shots) {
    if (only.length && !only.includes(shot.name)) continue;
    const signed = shot.login !== false && !shot.file;
    await take(signed ? page : signedOut, shot);
  }
} finally {
  await browser.close();
}
