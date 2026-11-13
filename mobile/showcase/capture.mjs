/*
 * Runs the phone app in Chrome against a pretend Mac and saves each screen.
 *
 *   pnpm showcase                 every scene into showcase/out
 *   pnpm showcase chat home       only those scenes
 *   pnpm showcase --site <dir>    also copy the captures there
 */
import { spawn, execFileSync } from 'node:child_process';
import { copyFile, mkdir } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { parseArgs } from 'node:util';
import { chromium } from '../../node_modules/playwright-core/index.mjs';

import { PANE_IMAGE_FILE } from './paneImage.ts';

const here = import.meta.dirname;
const app = resolve(here, '../app');
const CORE = 'showcase0000000000000000000000000000000000000000000000000000core';
const PANE = '/tmp/sikemux-phone-pane.jpg';

const { values: options, positionals: only } = parseArgs({
  allowPositionals: true,
  options: {
    out: { type: 'string', default: join(here, 'out') },
    port: { type: 'string', default: '8790' },
    site: { type: 'string' },
  },
});

const SCENES = [
  { name: 'home', route: '/' },
  { name: 'device', route: `/device/${CORE}` },
  // The demo host's list fits a whole phone, so a shorter screen gives it room to scroll under the tabs.
  { name: 'device-scrolled', route: `/device/${CORE}`, scroll: 400, height: 460 },
  { name: 'terminals', route: `/device/${CORE}`, tap: 'Terminals' },
  { name: 'calls', route: `/device/${CORE}?tab=calls` },
  { name: 'line-rows', route: `/device/${CORE}?line` },
  { name: 'line-chat', route: `/device/${CORE}/chat/chat-login-check?line` },
  { name: 'calls-in-place', route: `/device/${CORE}?tab=calls`, tap: 'Keep product pages cached for an hour?' },
  { name: 'chat', route: `/device/${CORE}/chat/chat-login` },
  { name: 'notes', route: `/device/${CORE}/chat/chat-notes` },
  { name: 'notes-top', route: `/device/${CORE}/chat/chat-notes`, scroll: -500 },
  { name: 'team', route: `/device/${CORE}/chat/chat-team` },
  { name: 'chief-away-top', route: `/device/${CORE}/chat/chat-team`, scroll: -220, scrollAt: 0.2 },
  { name: 'chief-away-mid', route: `/device/${CORE}/chat/chat-team`, scroll: -110, scrollAt: 0.2 },
  { name: 'chief-lines', route: `/device/${CORE}/chat/chat-team`, scroll: -560, scrollAt: 0.2 },
  { name: 'worker-told', route: `/device/${CORE}/chat/chat-refunds` },
  { name: 'asking', route: `/device/${CORE}/chat/chat-dark-mode` },
  { name: 'question', route: `/device/${CORE}/chat/chat-release` },
  { name: 'assumed', route: `/device/${CORE}/chat/chat-login-check` },
  { name: 'new-chat', route: `/device/${CORE}/new` },
  {
    name: 'work-opened',
    route: `/device/${CORE}/chat/chat-login-fix`,
    taps: [/^Worked for/, /ran tests|^5 tool calls/, /pnpm test src\/auth, failed/, /^(Edited|edit) LoginForm\.css/],
  },
  { name: 'live', route: `/device/${CORE}/chat/chat-signup-check` },
  { name: 'subagent', route: `/device/${CORE}/chat/chat-adapter-hunt`, taps: [/^Subagent/] },
  { name: 'prompt-strip', route: `/device/${CORE}/chat/chat-notes`, tapPart: 'Keep them short, and link each one.' },
  {
    name: 'prompt-edit',
    route: `/device/${CORE}/chat/chat-notes`,
    tapPart: 'Keep them short, and link each one.',
    tapLabel: 'Edit message',
  },
].filter((scene) => only.length === 0 || only.includes(scene.name));

const root = resolve(here, '../..');
execFileSync('node', ['scripts/generate.mjs'], { cwd: app, stdio: 'inherit' });
// The chats' markdown is read by the core's parser; built here, a chat would wait minutes on its first compile.
const markdownParser = execFileSync(
  join(root, 'scripts/bin/cargo'),
  ['build', '--quiet', '-p', 'sikemux-markdown', '--example', 'stdio', '--message-format=json'],
  { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'], maxBuffer: 1 << 28 },
)
  .split('\n')
  .filter(Boolean)
  .map((line) => JSON.parse(line))
  .find((message) => message.reason === 'compiler-artifact' && message.target?.name === 'stdio' && message.executable)?.executable;
if (!markdownParser) throw new Error('cargo built no markdown parser');

if (existsSync(PANE_IMAGE_FILE)) execFileSync('sips', ['-Z', '1600', PANE_IMAGE_FILE, '--out', PANE], { stdio: 'ignore' });

/** The status bar, Dynamic Island and home indicator iOS draws over an app. */
function drawPhoneChrome() {
  document.querySelector('#phone-chrome')?.remove();
  const chrome = document.createElement('div');
  chrome.id = 'phone-chrome';
  chrome.innerHTML = `
    <style>
      #phone-chrome { position: fixed; inset: 0; z-index: 99999; pointer-events: none; color: #fff;
        font: 600 17px/1 -apple-system, "SF Pro Text", system-ui, sans-serif; }
      #phone-chrome .bar { position: absolute; top: 0; left: 0; right: 0; height: 54px; display: flex;
        align-items: center; justify-content: space-between; padding: 6px 34px 0 52px; }
      #phone-chrome .island { position: absolute; top: 11px; left: 50%; width: 124px; height: 36px;
        margin-left: -62px; border-radius: 20px; background: #000; }
      #phone-chrome .icons { display: flex; align-items: center; gap: 7px; }
      #phone-chrome .home { position: absolute; bottom: 8px; left: 50%; width: 140px; height: 5px;
        margin-left: -70px; border-radius: 3px; background: rgba(255,255,255,0.92); }
    </style>
    <div class="bar">
      <span>9:41</span>
      <span class="icons">
        <svg width="19" height="12" viewBox="0 0 19 12" fill="#fff"><rect x="0" y="8" width="3.2" height="4" rx="1"/><rect x="5" y="5.5" width="3.2" height="6.5" rx="1"/><rect x="10" y="3" width="3.2" height="9" rx="1"/><rect x="15" y="0" width="3.2" height="12" rx="1"/></svg>
        <svg width="17" height="12" viewBox="0 0 17 12" fill="#fff"><path d="M8.5 2.3c2.4 0 4.6.9 6.2 2.4l1.2-1.2A10.4 10.4 0 0 0 8.5.6 10.4 10.4 0 0 0 1.1 3.5l1.2 1.2a8.7 8.7 0 0 1 6.2-2.4Zm0 3.4c1.5 0 2.8.6 3.8 1.5l1.2-1.2a7 7 0 0 0-5-2 7 7 0 0 0-5 2l1.2 1.2c1-.9 2.3-1.5 3.8-1.5Zm0 3.4c.6 0 1.1.2 1.5.6L8.5 11.2 7 9.7c.4-.4.9-.6 1.5-.6Z"/></svg>
        <svg width="27" height="13" viewBox="0 0 27 13" fill="none"><rect x=".5" y=".5" width="23" height="12" rx="3.8" stroke="#fff" stroke-opacity=".4"/><rect x="2" y="2" width="20" height="9" rx="2.5" fill="#fff"/><path d="M25 4.5v4c.8-.3 1.3-1.1 1.3-2s-.5-1.7-1.3-2Z" fill="#fff" fill-opacity=".45"/></svg>
      </span>
    </div>
    <div class="island"></div>
    <div class="home"></div>`;
  document.body.append(chrome);
}

/** Gives each icon's gradient its own id: in a page every `url(#id)` finds the first one, which may be hidden. */
function separateGradients() {
  document.querySelectorAll('svg').forEach((svg, index) => {
    for (const gradient of svg.querySelectorAll('[id]')) {
      const id = gradient.id;
      gradient.id = `${id}-${index}`;
      for (const user of svg.querySelectorAll(`[fill="url(#${id})"], [stroke="url(#${id})"]`))
        for (const attribute of ['fill', 'stroke'])
          if (user.getAttribute(attribute) === `url(#${id})`) user.setAttribute(attribute, `url(#${id}-${index})`);
    }
  });
}

const origin = `http://localhost:${options.port}`;
const server = spawn('npx', ['expo', 'start', '--web', '--port', options.port, '--clear'], {
  cwd: app,
  env: { ...process.env, CI: '1', SIKEMUX_SHOWCASE: '1', SIKEMUX_SHOWCASE_PANE: PANE, SIKEMUX_SHOWCASE_MARKDOWN: markdownParser },
  stdio: ['ignore', 'ignore', 'inherit'],
  detached: true,
});
const stop = () => {
  try {
    process.kill(-server.pid);
  } catch {}
};
process.on('exit', stop);

async function ready() {
  for (let attempt = 0; attempt < 400; attempt += 1) {
    try {
      if ((await fetch(origin, { signal: AbortSignal.timeout(5000) })).ok) return;
    } catch {}
    await new Promise((settle) => setTimeout(settle, 1000));
  }
  throw new Error(`the app never answered on ${origin}`);
}

await ready();
await mkdir(options.out, { recursive: true });
const browser = await chromium.launch({ channel: 'chrome' });
// An iPhone 17 Pro's screen in points, and the room its status bar and home indicator take.
const SCREEN = { width: 402, height: 874 };
const INSETS = { top: 62, bottom: 34, left: 0, right: 0 };

const page = await browser.newPage({
  viewport: SCREEN,
  deviceScaleFactor: 3,
  isMobile: true,
  hasTouch: true,
  colorScheme: 'dark',
});
const devtools = await page.context().newCDPSession(page);
await devtools.send('Emulation.setSafeAreaInsetsOverride', { insets: INSETS });
const problems = [];
page.on('pageerror', (error) => problems.push(error.message.split('\n')[0]));

for (const scene of SCENES) {
  await page.setViewportSize({ width: SCREEN.width, height: scene.height ?? SCREEN.height });
  await page.goto(origin + scene.route, { timeout: 180_000 });
  await page.waitForTimeout(3000);
  // On a cold server a chat can take longer than that pause to open.
  await page
    .waitForFunction(() => !document.body.innerText.includes('Opening the chat'), null, { timeout: 120_000 })
    .catch(() => problems.push(`${scene.name} never opened its chat`));
  if (scene.tap) {
    await page.getByText(scene.tap, { exact: true }).first().click();
    await page.waitForTimeout(800);
  }
  if (scene.tapPart) {
    await page.getByText(scene.tapPart).first().click();
    await page.waitForTimeout(800);
  }
  if (scene.tapLabel) {
    await page
      .getByLabel(scene.tapLabel)
      .first()
      .click({ timeout: 5000 })
      .catch(() => problems.push(`no ${scene.tapLabel} on ${scene.name}`));
    await page.waitForTimeout(800);
  }
  // Each opens the last thing labelled so, in order; one the screen does not have is passed over.
  for (const label of scene.taps ?? []) {
    await page
      .getByLabel(label)
      .last()
      .click({ timeout: 3000 })
      .catch(() => problems.push(`no ${label} on ${scene.name}`));
    await page.waitForTimeout(800);
  }
  if (scene.scroll) {
    await page.mouse.move(SCREEN.width / 2, (scene.height ?? SCREEN.height) * (scene.scrollAt ?? 0.7));
    await page.mouse.wheel(0, scene.scroll);
    await page.waitForTimeout(800);
  }
  await page.evaluate(separateGradients);
  await page.evaluate(drawPhoneChrome);
  const path = join(options.out, `${scene.name}.png`);
  await page.screenshot({ path });
  if (options.site) {
    await mkdir(options.site, { recursive: true });
    await copyFile(path, join(options.site, `phone-${scene.name}.png`));
  }
  console.log(`✓ ${scene.name}`);
}

await browser.close();
stop();
if (problems.length) {
  console.log(`\n${problems.length} problem(s):\n${problems.join('\n')}`);
  process.exitCode = 1;
}
process.exit();
