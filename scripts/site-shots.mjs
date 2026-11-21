#!/usr/bin/env node
// Take pictures of a built docs site (the `_site` that Jekyll writes). See
// .github/workflows/site-preview.yml.
//
// Serves the site on a loopback port the way GitHub Pages does, opens a fixed
// list of pages at 1440 and 390 px wide in the light colour scheme, and saves
// a first-screen and a full-page picture of each page and width:
// <name>-<width>-fold.png and <name>-<width>-full.png, and shots.json with
// the size of each page.
//
// Fails, with the page and the reason, on:
//   not-found       a page of the list that the site does not have
//   overflow-<w>    a page wider than its viewport at width w, so it scrolls
//                   sideways
//   stale-known     an overflow entry of --known that matches no failure
//
// --known is the file of scripts/site-check.py, one `page::rule  # reason`
// per line. Only its `overflow-<width>` entries are read here.
//
// Playwright comes from web/node_modules: run `npm ci` and
// `npx playwright install chromium` in web first.
//
// Usage: node scripts/site-shots.mjs <site dir> <out dir> [--known FILE]

import {
  createReadStream,
  existsSync,
  mkdirSync,
  readFileSync,
  statSync,
  writeFileSync,
} from 'node:fs';
import { createServer } from 'node:http';
import { dirname, extname, join, resolve, sep } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const PLAYWRIGHT = join(ROOT, 'web', 'node_modules', 'playwright', 'index.mjs');

// The pages to shoot: picture name and the path the site serves it at.
const PAGES = [
  ['landing', '/'],
  ['get-started', '/get-started/'],
  ['docs-home', '/guides/'],
  ['guide', '/guides/observability-setup.html'],
  ['walkthrough', '/guides/platform-walkthrough.html'],
  ['reference', '/reference/kronika-pages.html'],
  ['releases', '/releases/'],
];
const VIEWPORTS = [
  { width: 1440, height: 900 },
  { width: 390, height: 844 },
];
const QUIET_MS = 15000;
const TYPES = {
  '.html': 'text/html; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.mjs': 'text/javascript; charset=utf-8',
  '.json': 'application/json',
  '.cast': 'application/x-asciicast',
  '.svg': 'image/svg+xml',
  '.png': 'image/png',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.gif': 'image/gif',
  '.webp': 'image/webp',
  '.ico': 'image/x-icon',
  '.woff': 'font/woff',
  '.woff2': 'font/woff2',
  '.ttf': 'font/ttf',
  '.mp4': 'video/mp4',
  '.webm': 'video/webm',
  '.txt': 'text/plain; charset=utf-8',
  '.xml': 'application/xml',
};
const KNOWN_LINE = /^(\S+)::([a-z0-9-]+)\s+#\s*(\S.*)$/;
const STILL =
  '*,*::before,*::after{animation:none!important;transition:none!important}' +
  'html{scroll-behavior:auto!important}';

const isFile = (path) => existsSync(path) && statSync(path).isFile();
const isDir = (path) => existsSync(path) && statSync(path).isDirectory();

// The site path of a page path: `/` is `index.html`, `/a/` is `a/index.html`.
function sitePath(path) {
  return path.endsWith('/') ? `${path.slice(1)}index.html` : path.slice(1);
}

// What GitHub Pages answers a path with: { file }, { redirect } or {}.
function lookup(site, path) {
  const target = resolve(site, `.${decodeURIComponent(path)}`);
  if (target !== site && !target.startsWith(site + sep)) return {};
  if (path.endsWith('/')) {
    return isFile(join(target, 'index.html')) ? { file: join(target, 'index.html') } : {};
  }
  if (isFile(target)) return { file: target };
  if (isFile(`${target}.html`)) return { file: `${target}.html` };
  return isFile(join(target, 'index.html')) ? { redirect: true } : {};
}

function answer(site, request, response) {
  const { pathname } = new URL(request.url, 'http://127.0.0.1');
  let found;
  try {
    found = lookup(site, pathname);
  } catch (error) {
    response.writeHead(400, { 'content-type': 'text/plain' }).end(`bad path: ${error.message}`);
    return;
  }
  if (found.redirect) {
    response.writeHead(301, { location: `${pathname}/` }).end();
    return;
  }
  const missing = join(site, '404.html');
  const file = found.file ?? (isFile(missing) ? missing : null);
  if (!file) {
    response.writeHead(404, { 'content-type': 'text/plain' }).end('not found');
    return;
  }
  const type = TYPES[extname(file).toLowerCase()] ?? 'application/octet-stream';
  response.writeHead(found.file ? 200 : 404, { 'content-type': type });
  createReadStream(file).pipe(response);
}

function serve(site) {
  const server = createServer((request, response) => answer(site, request, response));
  return new Promise((ready, failed) => {
    server.once('error', failed);
    server.listen(0, '127.0.0.1', () => ready(server));
  });
}

// Runs in the page: its size and, when it is too wide, the element that sticks
// out furthest and is not inside a box that scrolls or clips.
function measure() {
  const root = document.documentElement;
  const size = {
    width: root.scrollWidth,
    height: root.scrollHeight,
    viewportWidth: root.clientWidth,
    widest: null,
  };
  if (size.width <= size.viewportWidth) return size;
  const clipped = (element) => {
    for (let box = element.parentElement; box && box !== document.body; box = box.parentElement) {
      if (getComputedStyle(box).overflowX !== 'visible') return true;
    }
    return false;
  };
  let right = root.clientWidth;
  for (const element of document.body.querySelectorAll('*')) {
    const edge = element.getBoundingClientRect().right;
    if (edge <= right + 0.5 || clipped(element)) continue;
    right = edge;
    size.widest = [element.tagName.toLowerCase(), ...element.classList].join('.');
  }
  return size;
}

async function shoot(context, base, out, [name, path], { width }) {
  const page = await context.newPage();
  await page.goto(base + path, { waitUntil: 'load' });
  await page.waitForLoadState('networkidle', { timeout: QUIET_MS }).catch((error) => {
    console.log(
      `${sitePath(path)}: the network was not quiet after ${QUIET_MS} ms, shooting anyway: ${error.message.split('\n')[0]}`,
    );
  });
  await page.addStyleTag({ content: STILL });
  await page.evaluate(() => document.fonts.ready);
  await page.screenshot({ path: join(out, `${name}-${width}-fold.png`) });
  await page.screenshot({ path: join(out, `${name}-${width}-full.png`), fullPage: true });
  const size = await page.evaluate(measure);
  await page.close();
  console.log(`shot ${name} at ${width}: ${size.width} x ${size.height} px`);
  return { page: sitePath(path), name, viewport: width, ...size };
}

async function shootAll(site, out) {
  const { chromium } = await import(pathToFileURL(PLAYWRIGHT).href);
  const server = await serve(site);
  const base = `http://127.0.0.1:${server.address().port}`;
  const browser = await chromium.launch();
  const shots = [];
  try {
    for (const viewport of VIEWPORTS) {
      const context = await browser.newContext({
        viewport,
        colorScheme: 'light',
        deviceScaleFactor: 1,
        reducedMotion: 'reduce',
      });
      for (const entry of PAGES) shots.push(await shoot(context, base, out, entry, viewport));
      await context.close();
    }
  } finally {
    await browser.close();
    server.close();
  }
  return shots;
}

// The overflow entries of a known file: `page::overflow-<width>` to its line.
function readKnown(file) {
  const entries = new Map();
  if (!file) return entries;
  readFileSync(file, 'utf8')
    .split('\n')
    .forEach((row, index) => {
      const entry = KNOWN_LINE.exec(row.trim());
      if (entry && /^overflow-\d+$/.test(entry[2])) {
        entries.set(`${entry[1]}::${entry[2]}`, index + 1);
      }
    });
  return entries;
}

function overflowFailures(shots, known, knownFile) {
  const failures = [];
  const used = new Set();
  for (const shot of shots) {
    if (shot.width <= shot.viewportWidth) continue;
    const key = `${shot.page}::overflow-${shot.viewport}`;
    const widest = shot.widest ? `, the widest element is ${shot.widest}` : '';
    const line = `${shot.page}: overflow-${shot.viewport}: ${shot.width} px wide on a ${shot.viewport} px screen${widest}`;
    if (!known.has(key)) {
      failures.push(line);
      continue;
    }
    used.add(key);
    console.log(`known: ${line}`);
  }
  for (const [key, number] of known) {
    if (used.has(key)) continue;
    failures.push(
      `${knownFile}:${number}: stale-known: \`${key}\` matches no failure; delete the line`,
    );
  }
  return failures;
}

async function main(argv) {
  const { values, positionals } = parseArgs({
    args: argv,
    allowPositionals: true,
    options: { known: { type: 'string' } },
  });
  if (positionals.length !== 2) {
    console.error('usage: node scripts/site-shots.mjs <site dir> <out dir> [--known FILE]');
    return 2;
  }
  const [site, out] = positionals.map((path) => resolve(path));
  if (!isDir(site)) {
    console.error(`site-shots: not a dir: ${site}`);
    return 2;
  }
  if (values.known && !isFile(values.known)) {
    console.error(`site-shots: not a file: ${values.known}`);
    return 2;
  }
  const missing = PAGES.filter(([, path]) => !lookup(site, path).file);
  for (const [, path] of missing) {
    console.log(
      `${sitePath(path)}: not-found: the page is not in the site; update PAGES in scripts/site-shots.mjs`,
    );
  }
  if (missing.length) return 1;
  mkdirSync(out, { recursive: true });
  const shots = await shootAll(site, out);
  writeFileSync(join(out, 'shots.json'), `${JSON.stringify(shots, null, 2)}\n`);
  const failures = overflowFailures(shots, readKnown(values.known), values.known);
  for (const line of failures) console.log(line);
  if (failures.length) {
    console.log(`${failures.length} picture failure(s).`);
    return 1;
  }
  console.log(`Pictures saved: ${shots.length * 2} in ${out}.`);
  return 0;
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
