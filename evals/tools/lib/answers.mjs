import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';
import { tmpdir } from 'node:os';
import { basename, dirname, isAbsolute, join } from 'node:path';
import { deflateSync, crc32 } from 'node:zlib';
import { valueMatches, toolMatches } from './match.mjs';
import { AUTHOR, BASE_PATH } from './sandbox.mjs';
import { readRepo } from './surface.mjs';

const pictures = new Map();

// A plain picture at the size the real tool returns, so a screenshot does not look broken.
function picture(width, height) {
  const key = `${width}x${height}`;
  if (pictures.has(key)) return pictures.get(key);
  const row = Buffer.alloc(1 + width * 3);
  const rows = [];
  for (let y = 0; y < height; y += 1) {
    const shade = y < 48 ? 0x2a : 0xf4;
    row.fill(shade, 1);
    rows.push(Buffer.from(row));
  }
  const chunk = (type, data) => {
    const length = Buffer.alloc(4);
    length.writeUInt32BE(data.length);
    const body = Buffer.concat([Buffer.from(type), data]);
    const sum = Buffer.alloc(4);
    sum.writeUInt32BE(crc32(body));
    return Buffer.concat([length, body, sum]);
  };
  const header = Buffer.alloc(13);
  header.writeUInt32BE(width, 0);
  header.writeUInt32BE(height, 4);
  header.set([8, 2, 0, 0, 0], 8);
  const png = Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', header),
    chunk('IDAT', deflateSync(Buffer.concat(rows))),
    chunk('IEND', Buffer.alloc(0)),
  ]).toString('base64');
  pictures.set(key, png);
  return png;
}


const BLANK_PAGE = { url: 'about:blank', title: '', elements: [], text: '' };

export function answerer(fixture, project, previewProfile) {
  const rules = (fixture.answers ?? []).map((rule) => ({ ...rule, used: 0 }));
  const pages = fixture.pages ?? [];
  const me = fixture.me ?? 'Juniper';
  const agents = [
    { name: me, agentId: 'agent-eval-self', cli: 'claude', status: 'working', you: true },
    ...(fixture.agents ?? []).map((agent, index) => ({
      agentId: `agent-eval-${index + 1}`,
      cli: 'claude',
      status: 'idle',
      canMessage: true,
      ...agent,
    })),
  ];
  const state = { tabs: [], active: null, tabCount: 0, executions: 0, cursor: 100 };
  for (const url of [fixture.tab, ...(fixture.tabs ?? [])].filter(Boolean)) openTab(state, findPage(pages, url, url));
  state.active = state.tabs[0] ?? null;

  return (tool, args) => {
    if (tool === 'browser_viewport' && state.active) state.active.viewport = sized(args);
    const rule = rules.find(
      (candidate) =>
        toolMatches(candidate.tool, tool) &&
        (!candidate.args || valueMatches(candidate.args, args)) &&
        (candidate.times === undefined || candidate.used < candidate.times),
    );
    if (rule) {
      rule.used += 1;
      if (rule.page) openTab(state, findPage(pages, rule.page, rule.page), !args.newTab);
      const fill = (value) => fillIn(value, { ...args, project });
      if (rule.error) return { error: fill(rule.error) };
      return { value: fill(rule.reply ?? {}) };
    }
    return builtIn(tool, args ?? {}, { state, pages, agents, project, fixture, previewProfile });
  };
}

function fillIn(value, args) {
  if (typeof value === 'string') return value.replace(/\{\{(\w+)\}\}/g, (whole, key) => (key in args ? String(args[key]) : whole));
  if (Array.isArray(value)) return value.map((item) => fillIn(item, args));
  if (value && typeof value === 'object') return Object.fromEntries(Object.entries(value).map(([key, item]) => [key, fillIn(item, args)]));
  return value;
}

function numbered(page, index) {
  return elementsText(page)
    .split('\n')
    .find((line) => line.startsWith(`[${index}] `));
}

function shows(page, text) {
  const wanted = String(text).toLowerCase();
  return `${elementsText(page)}\n${page.text ?? ''}`.toLowerCase().includes(wanted);
}

function findPage(pages, url, fallbackUrl) {
  const page = pages.find((candidate) => valueMatches(candidate.url, url) || candidate.url === url);
  return page ? { ...BLANK_PAGE, ...page, url: page.href ?? url } : { ...BLANK_PAGE, url: fallbackUrl, title: fallbackUrl };
}

function openTab(state, page, replace = false) {
  if (replace && state.active) {
    state.active.page = page;
    return state.active;
  }
  const tab = { tabId: `browser-agent-eval-${state.tabCount++}`, page };
  state.tabs.push(tab);
  state.active = tab;
  return tab;
}

function tabOf(state, args) {
  if (args.tabId) return state.tabs.find((tab) => tab.tabId === args.tabId);
  return state.active;
}

function elementsText(page) {
  return page.elements.map((line, index) => (line.startsWith('[') ? line : `[${index}] ${line}`)).join('\n');
}

function where(tab) {
  return { tabId: tab.tabId, url: tab.page.url, title: tab.page.title, loading: false };
}

function pageState(tab, full) {
  return {
    ...where(tab),
    scroll: { y: tab.scrollY ?? 0, height: 2400, viewport: 900 },
    viewport: { width: 1280, height: 900 },
    elements: elementsText(tab.page),
    ...(full && tab.page.text ? { text: tab.page.text } : {}),
  };
}

const NO_TAB = 'No desk tab is open for this agent. Open one with browser_navigate.';

function builtIn(tool, args, { state, pages, agents, project, fixture, previewProfile }) {
  const tab = tabOf(state, args);
  const needTab = (answer) => (tab ? { value: answer() } : { error: NO_TAB });
  const outcome = args.report === 'outcome';
  switch (tool) {
    case 'browser_navigate': {
      if (!args.url && args.go && !tab) return { error: NO_TAB };
      const page = args.url ? findPage(pages, args.url, args.url) : tab?.page ?? findPage(pages, 'about:blank', 'about:blank');
      const opened = openTab(state, page, !args.newTab);
      const met = args.waitFor ? { met: true, waitedMs: 240 } : {};
      return { value: outcome ? { ...met, ...where(opened) } : { ...met, ...pageState(opened, false) } };
    }
    case 'browser_state':
      return needTab(() => pageState(tab, args.fullText));
    case 'browser_find':
      return needTab(() => {
        const wanted = String(args.query ?? args.text ?? '').toLowerCase();
        const lines = elementsText(tab.page)
          .split('\n')
          .filter((line) => line && (!wanted || line.toLowerCase().includes(wanted)));
        return lines.length
          ? { matches: lines.length, elements: lines.join('\n') }
          : { matches: 0, elements: '', note: 'nothing on the page shows those words' };
      });
    case 'browser_click':
      if (tab && [args.index, args.text, args.selector, args.role, args.x].every((value) => value === undefined)) return { error: 'pass index, text or selector' };
      return needTab(() => ({
        ...(args.index !== undefined ? { index: args.index } : {}),
        label: args.text ?? args.selector ?? numbered(tab.page, args.index)?.replace(/^\[\d+\] /, '') ?? 'element',
        action: 'clicked',
        ...(outcome ? where(tab) : pageState(tab, false)),
      }));
    case 'browser_type':
      return needTab(() => ({
        typed: String(args.text ?? '').length,
        into: args.selector ?? numbered(tab.page, args.index)?.replace(/^\[\d+\] /, '') ?? 'field',
        value: args.text,
        submitted: Boolean(args.submit),
        ...where(tab),
      }));
    case 'browser_press':
      return needTab(() => ({ pressed: args.key, ...where(tab) }));
    case 'browser_act':
      return needTab(() => ({ steps: (args.steps ?? []).length, done: true, ...where(tab) }));
    case 'browser_scroll':
      return needTab(() => {
        const y = args.to === 'bottom' ? 1500 : args.to === 'top' ? 0 : Math.max(0, Math.min(1500, (tab.scrollY ?? 0) + (args.deltaY ?? 600)));
        tab.scrollY = y;
        return { y, height: 2400, viewport: 900, atBottom: y >= 1500 };
      });
    case 'browser_wait':
      return needTab(() => {
        const met =
          (args.text === undefined || shows(tab.page, args.text)) &&
          (args.textGone === undefined || !shows(tab.page, args.textGone)) &&
          (args.url === undefined || tab.page.url.includes(args.url)) &&
          (args.urlGone === undefined || !tab.page.url.includes(args.urlGone));
        return { met, waitedMs: met ? 320 : args.timeoutMs ?? 10000, ...where(tab) };
      });
    case 'browser_evaluate':
      return needTab(() => ({ result: null }));
    case 'browser_screenshot':
      return needTab(() => ({ ...screenshot(tab, args, previewProfile), title: tab.page.title, url: tab.page.url }));
    case 'browser_viewport':
      return needTab(() => ({ viewport: { ...sized(args), preset: args.preset ?? null }, ...where(tab) }));
    case 'browser_switch_tab': {
      const target = state.tabs.find((candidate) => candidate.tabId === args.tabId);
      if (args.tabId && !target) return { error: `No tab ${args.tabId}` };
      if (target) state.active = target;
      return { value: { activeTabId: state.active?.tabId ?? null, tabs: state.tabs.map((one) => where(one)) } };
    }
    case 'browser_close_tab':
      state.tabs = state.tabs.filter((candidate) => candidate.tabId !== (args.tabId ?? state.active?.tabId));
      state.active = state.tabs.at(-1) ?? null;
      return { value: { activeTabId: state.active?.tabId ?? null, tabs: state.tabs.map((one) => where(one)) } };
    case 'browser_console':
      return needTab(() => ({ recording: true, url: tab.page.url, recorded: 0, matched: 0, messages: [] }));
    case 'browser_network':
      return needTab(() => ({ recording: true, recorded: 0, requests: [] }));
    case 'browser_upload':
      return needTab(() => ({ uploaded: (args.paths ?? []).map((path) => path.split('/').at(-1)), ...where(tab) }));
    case 'browser_dialog':
      return needTab(() => ({ dialog: null }));
    case 'page_preview':
      return preview(args, project, previewProfile);
    case 'page_show':
      return {
        value: {
          page: { id: 'page-eval-1', title: args.title ?? 'Page', height: args.height ?? 900 },
          note: 'The person sees the page in your reply, above the text you write next. Do not describe or restate it; write only what it does not already say.',
        },
      };
    case 'process_start': {
      const executionId = `exec-eval-${++state.executions}`;
      return { value: { executionId, processId: `sh:${args.label ?? 'process'}`, project, status: 'running', label: args.label ?? args.command, command: args.command, ready: false } };
    }
    case 'process_read':
      return { value: { executionId: args.executionId, status: 'exited', exitCode: 0, output: fixture.processOutput ?? 'done\n' } };
    case 'process_stop':
    case 'process_restart':
      return { value: { executionId: args.executionId, status: tool === 'process_stop' ? 'stopped' : 'running' } };
    case 'events_wait':
      return {
        value: {
          events: [{ kind: 'process.exited', executionId: args.executionId ?? 'exec-eval-1', exitCode: 0 }],
          cursor: String(++state.cursor),
        },
      };
    case 'agent_list':
      return { value: { project, agents } };
    case 'agent_send': {
      const wanted = String(args.to);
      const agent = agents.find((one) => one.name.toLowerCase() === wanted.toLowerCase() || one.agentId === wanted);
      if (agent?.you) return { error: 'That agent is you' };
      if (!agent) return { error: `No open agent is called ${wanted}; agent_list shows the agents you can message` };
      return { value: { to: agent.name, agentId: agent.agentId, delivery: 'newTurn', note: `${agent.name} was idle and started a turn to read this.` } };
    }
    case 'agent_spawn':
      return {
        value: {
          name: 'Fern',
          agentId: 'agent-eval-spawned',
          cli: args.cli ?? 'claude',
          reportsBack: true,
          note: 'Fern is starting in its own tab and gets your prompt once it is ready. Each time its turn ends, its final answer comes to you as a message.',
        },
      };
    case 'agent_read':
      return { value: { name: args.agent, turns: [], status: 'idle' } };
    case 'agent_sessions':
      return { value: { sessions: [] } };
    case 'agent_wait':
      return { value: { events: [], cursor: String(++state.cursor), timedOut: true, note: 'Nothing you wait for has happened yet. Wait again with this cursor, or carry on: an agent you started reports to you by itself.' } };
    case 'who_wrote':
      return { value: { target: 'file', path: args.target, commits: [], owners: [{ kind: 'person', name: 'the person', action: 'ask the person' }], note: '' } };
    case 'git_commit':
      return commit(project, args, fixture.yours ?? {});
    case 'ask':
      return {
        value: {
          questionId: 'question-eval-1',
          askedOf: fixture.lead ? fixture.lead : 'the person',
          note: args.blocked === false ? 'Carry on with your assumption; a message tells you if it is wrong.' : 'Asked. End your turn now; the answer comes to you as a message.',
        },
      };
    case 'app_console':
      return { value: { recorded: 0, matched: 0, messages: [] } };
    case 'workspace_inspect':
      return { value: { project, roots: [project], processes: [] } };
    case 'ui_open':
      return { value: { opened: args.kind ?? 'file' } };
    case 'board_list':
      return { value: { cards: [] } };
    case 'board_add':
      return { value: { card: 1, title: args.title, status: 'open' } };
    case 'sim_devices':
      return { value: { devices: [{ udid: 'SIM-EVAL-1', name: 'iPhone 17', state: 'Booted', runtime: 'iOS 26.0' }] } };
    case 'sim_screenshot':
      return { value: { data: picture(390, 844), mimeType: 'image/png', title: 'iPhone 17', url: '' } };
    case 'sim_state':
      return { value: { app: 'com.example.app', elements: elementsText(fixture.sim ?? BLANK_PAGE) } };
    case 'jira_search':
      return { value: { issues: [], next: null } };
    case 'jira_issue':
      return { value: { key: args.key ?? 'PROJ-1', summary: 'Ticket', status: 'To Do', description: '' } };
    case 'jira_comment':
    case 'jira_edit':
    case 'jira_transition':
    case 'jira_link':
    case 'jira_worklog':
      return { value: { ok: true, key: args.key ?? args.issue ?? null } };
    case 'jira_create':
      return { value: { key: 'PROJ-101', url: 'https://example.atlassian.net/browse/PROJ-101' } };
    default:
      return { value: { ok: true } };
  }
}

// Commits for real in the case's repo, so an agent that checks the result finds the commit it was told about.
function commit(repo, args, yours) {
  if (/co-authored-by:[^\n]*claude/i.test(args.message ?? '')) {
    return { error: "Nothing was committed: leave out the Co-Authored-By trailer naming Claude. Sikemux's footer credits the agent already." };
  }
  const git = (...words) => execFileSync('/usr/bin/git', words, { cwd: repo, env: { ...AUTHOR, PATH: BASE_PATH }, encoding: 'utf8' });
  const paths = (args.paths ?? []).map((path) => (path.startsWith(`${repo}/`) ? path.slice(repo.length + 1) : path));
  const own = (path) => yours[path] !== undefined;
  const committed = (path) => {
    try {
      return git('show', `HEAD:${path}`);
    } catch {
      return null;
    }
  };
  const changed = paths.some(own)
    ? paths.filter((path) => !own(path) || committed(path) !== yours[path]).join(' ')
    : git('status', '--porcelain', '--', ...paths).trim();
  if (!changed) {
    if (args.land && !args.message) return { value: { landed: true, branch: 'refs/heads/main', commits: [] } };
    return { error: paths.length ? `Nothing was committed: ${paths.join(', ')} ${paths.length === 1 ? 'has' : 'have'} nothing uncommitted in ${repo}.` : 'None of the uncommitted lines here are yours' };
  }
  const plain = paths.filter((path) => !own(path));
  if (plain.length || !paths.length) git('add', '--', ...(paths.length ? plain : ['.']));
  for (const path of paths.filter(own)) {
    const blob = execFileSync('/usr/bin/git', ['hash-object', '-w', '--stdin'], { cwd: repo, input: yours[path], encoding: 'utf8' }).trim();
    git('update-index', '--add', '--cacheinfo', `100644,${blob},${path}`);
  }
  git('commit', '-q', '-m', args.message || 'Update');
  const files = git('show', '--numstat', '--format=', 'HEAD')
    .split('\n')
    .filter(Boolean)
    .map((line) => {
      const [added, removed, path] = line.split('\t');
      return { path, added: Number(added), removed: Number(removed) };
    });
  const left = git('status', '--porcelain', '-z').split('\0').filter(Boolean).map((entry) => entry.replace(/^.. /, ''));
  return { value: { commit: git('rev-parse', 'HEAD').trim(), branch: 'refs/heads/main', files, left, ...(args.land ? { landed: true } : {}) } };
}

// A dark theme with the variables Sikemux sets on a page's :root; the real values follow the person's theme.
const THEME = {
  '--background': '#16161d',
  '--foreground': '#e8e8ef',
  '--muted-foreground': '#a0a0ad',
  '--faint-foreground': '#6c6c78',
  '--card': '#1f1f28',
  '--border': 'rgba(255,255,255,0.1)',
  '--accent': '#a277ff',
  '--accent-foreground': '#ffffff',
  '--accent-surface': 'rgba(162,119,255,0.16)',
  '--destructive': '#f0616d',
  '--success': '#61d095',
  '--warning': '#ffca85',
  '--info': '#82b1ff',
  '--code-background': '#15141b',
  '--code-foreground': '#edecee',
  '--chart-1': '#a277ff',
  '--chart-2': '#82b1ff',
  '--chart-3': '#61d095',
  '--chart-4': '#ffca85',
  '--chart-5': '#f694ff',
  '--chart-6': '#61ffca',
  '--radius': '8px',
  '--font-sans': 'Figtree, system-ui',
  '--font-mono': 'JetBrains Mono, ui-monospace',
};

// The page as Sikemux draws it: its theme and base styles ahead of everything the agent wrote.
function themed(path) {
  const base = readRepo('crates/sikemux-base/src/pages.rs').match(/const BASE_CSS: &str = "([^"]+)";/)[1];
  const theme = `:root{color-scheme:dark;${Object.entries(THEME).map(([name, value]) => `${name}:${value};`).join('')}}`;
  const head = `<meta charset="utf-8"><base href="${pathToFileURL(dirname(path)).href}/"><style id="sikemux-theme">${theme}</style><style>${base}</style>`;
  return head + readFileSync(path, 'utf8');
}

// Draws the agent's page in an offscreen WebKit view, as Sikemux does, inside the agent's sandbox.
function preview(args, repo, profile) {
  const home = dirname(dirname(repo));
  const path = args.path.startsWith('~/') ? join(home, args.path.slice(2)) : isAbsolute(args.path) ? args.path : join(repo, args.path);
  if (!existsSync(path)) return { error: `${args.path} could not be read` };
  const width = args.width ?? 756;
  const folder = mkdtempSync(join(tmpdir(), 'preview-'));
  const copy = join(folder, basename(path));
  const shot = join(folder, 'shot.png');
  writeFileSync(copy, themed(path));
  try {
    const drawn = draw(profile, copy, width, args.picture === false ? '' : shot, args.script);
    if (drawn.error) return { error: drawn.error };
    const value = {
      width,
      contentHeight: drawn.contentHeight ?? 900,
      console: { recording: true, recorded: drawn.messages?.length ?? 0, matched: drawn.messages?.length ?? 0, messages: drawn.messages ?? [] },
      ...(args.script ? { script: drawn.script ?? null } : {}),
    };
    if (args.picture === false || !existsSync(shot)) return { value };
    return { value: { data: readFileSync(shot).toString('base64'), mimeType: 'image/png', ...value } };
  } finally {
    rmSync(folder, { recursive: true, force: true });
  }
}

const PREVIEWERS = '/private/tmp/sikemux-tool-eval-previewers';

function draw(profile, page, width, shot, script = '', tallest = 14_400) {
  const output = execFileSync('/usr/bin/sandbox-exec', ['-f', profile, previewer(), page, '/', String(width), shot, script, String(tallest)], {
    encoding: 'utf8',
    timeout: 30_000,
    stdio: ['ignore', 'pipe', 'ignore'],
  });
  return JSON.parse(output || '{}');
}

const PRESETS = { desktop: [1280, 800], tablet: [820, 1180], mobile: [390, 844] };

function sized(args) {
  const [width, height] = PRESETS[args.preset] ?? [args.width ?? 1280, args.height ?? 800];
  return { width: args.width ?? width, height: args.height ?? height };
}

const escape = (text) => String(text).replace(/[&<>"]/g, (char) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' })[char]);

// A plain drawing of a fixture page, from its own html or from its elements and text.
function pageHtml(page) {
  if (page.html) return page.html;
  const parts = page.elements.map((line) => {
    const [, tag = 'div', label = ''] = line.replace(/^\[\d+\]\s*/, '').match(/^<(\w+)[^>]*>\s*(.*)$/) ?? [null, 'div', line];
    const text = escape(label.replace(/\s*\([^)]*\)$/, ''));
    if (/^h[1-6]$/.test(tag)) return `<${tag}>${text}</${tag}>`;
    if (tag === 'button') return `<p><button>${text}</button></p>`;
    if (tag === 'a') return `<p><a href="#">${text}</a></p>`;
    if (tag === 'input' || tag === 'textarea') return `<p><input placeholder="${text}" style="width:60%"></p>`;
    if (tag === 'img') return `<div style="width:240px;height:120px;background:#ccd" title="${text}"></div>`;
    return `<p>${text}</p>`;
  });
  const prose = (page.text ?? '').split(/\n+/).filter(Boolean).map((line) => `<p>${escape(line)}</p>`);
  return `<!doctype html><title>${escape(page.title)}</title><body style="font:15px -apple-system, sans-serif;margin:24px;color:#1d1d1f;background:#fff">${[...parts, ...prose].join('')}</body>`;
}

function screenshot(tab, args, profile) {
  const { width, height } = tab.viewport ?? sized({});
  const folder = mkdtempSync(join(tmpdir(), 'shot-'));
  try {
    const page = join(folder, 'page.html');
    const shot = join(folder, 'shot.png');
    writeFileSync(page, pageHtml(tab.page));
    draw(profile, page, width, shot, '', args.fullPage ? 14_400 : height);
    return { data: readFileSync(shot).toString('base64'), mimeType: 'image/png' };
  } catch {
    return { data: picture(width, height), mimeType: 'image/png' };
  } finally {
    rmSync(folder, { recursive: true, force: true });
  }
}

// The WebKit previewer, compiled once for each version of its source.
function previewer() {
  const source = join(import.meta.dirname, 'preview.swift');
  mkdirSync(PREVIEWERS, { recursive: true });
  const binary = join(PREVIEWERS, `page-preview-${createHash('sha256').update(readFileSync(source)).digest('hex').slice(0, 12)}`);
  if (!existsSync(binary)) execFileSync('/usr/bin/xcrun', ['swiftc', '-O', source, '-o', binary], { stdio: 'ignore' });
  return binary;
}
