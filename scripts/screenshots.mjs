// Regenerates the README screenshots in docs/screenshots/.
//
// It runs the frontend through cmd/devserver and captures it with headless
// Chrome via Playwright:
//
//   PLAYWRIGHT=/path/to/node_modules/playwright/index.mjs \
//   CHROME_PATH=/path/to/chrome \
//   node scripts/screenshots.mjs
//
// PLAYWRIGHT defaults to the "playwright" package; CHROME_PATH to
// Playwright's own browser. For Windows-like text on Linux, install the
// Selawik font and alias "Segoe UI" to it in fontconfig, and install
// Cascadia Code.

import { spawn } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const out = path.join(root, 'docs', 'screenshots');
const port = 8097;
const url = `http://127.0.0.1:${port}/`;

const { chromium } = await import(process.env.PLAYWRIGHT || 'playwright');

const server = spawn('go', ['run', './cmd/devserver', '-addr', `127.0.0.1:${port}`,
  'docs/screenshots/kepler-track.md', 'docs/screenshots/gear-list.md'], { cwd: root, stdio: 'inherit', detached: true });

// go run starts the server as a child process, so stop the whole group.
const stopServer = () => {
  try { process.kill(-server.pid); } catch { /* already gone */ }
};

async function waitForServer() {
  for (let i = 0; i < 120; i++) {
    try {
      if ((await fetch(url)).ok) return;
    } catch { /* not up yet */ }
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error('devserver did not start');
}

const settle = (page, ms = 600) => page.waitForTimeout(ms);

// scrollTo puts a heading near the top of the active tab.
async function scrollTo(page, id, offset = 24) {
  await page.evaluate(([id, offset]) => {
    const sc = document.querySelector('.scroller:not([hidden])');
    const el = document.getElementById(id);
    sc.scrollTop += el.getBoundingClientRect().top - sc.getBoundingClientRect().top - offset;
  }, [id, offset]);
  await settle(page);
}

async function shot(page, name) {
  await page.mouse.move(1100, 20); // park the pointer on the empty tab strip
  await settle(page, 300);
  await page.screenshot({ path: path.join(out, name) });
  console.log('wrote', name);
}

try {
  await waitForServer();
  const browser = await chromium.launch({ executablePath: process.env.CHROME_PATH || undefined });
  const page = await browser.newPage({ viewport: { width: 1280, height: 800 }, deviceScaleFactor: 2 });
  page.on('pageerror', (e) => console.error('page error:', e.message));
  await page.goto(url);
  await page.waitForFunction(() => document.querySelectorAll('.tab').length === 2);
  await page.keyboard.press('Control+1'); // the trip plan
  await page.waitForSelector('.scroller:not([hidden]) .diagram svg');
  await page.waitForSelector('.scroller:not([hidden]) .katex');
  await settle(page, 800);

  // Dark theme, top of the document.
  await shot(page, 'dark.png');

  // Diagram and maths.
  await scrollTo(page, 'route-overview', -150);
  await shot(page, 'diagrams.png');

  // Find with highlighted matches.
  await scrollTo(page, 'day-by-day');
  await page.keyboard.press('Control+f');
  await page.keyboard.type('hut');
  await page.waitForFunction(() => /of/.test(document.getElementById('find-count').textContent));
  await settle(page);
  await shot(page, 'find.png');
  await page.keyboard.press('Escape');

  // Settings, over the timings.
  await scrollTo(page, 'timings');
  await page.click('#btn-settings');
  await settle(page, 300);
  await shot(page, 'settings.png');
  await page.keyboard.press('Escape');

  // Light theme.
  await page.keyboard.press('Control+Shift+D');
  await page.waitForFunction(() => document.documentElement.dataset.theme === 'light');
  await settle(page, 1200); // diagrams redraw
  await scrollTo(page, 'timings');
  await shot(page, 'light.png');
  await page.keyboard.press('Control+Shift+D'); // back to dark for next time

  await browser.close();
} finally {
  stopServer();
}
