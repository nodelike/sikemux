/*
 * The Mac the phone showcase pretends to be paired with: its projects, agents,
 * terminals and chats, written to read well in a screenshot.
 */

import type { ChatMessage, ChatPart, ChatView, PermissionAsk, ToolAction, ToolCall } from '../../src/chat/types';

export const CORE = 'showcase0000000000000000000000000000000000000000000000000000core';
export const HOST = { name: 'MacBook Pro', model: 'MacBook Pro' };
export const PANE_PICTURE = '/showcase/pane.jpg';

const HOME = '/Users/edon/code';

export const PROJECTS = [
  { id: 'p-shop', name: 'acme-shop', path: `${HOME}/acme-shop` },
  { id: 'p-site', name: 'marketing-site', path: `${HOME}/marketing-site` },
  { id: 'p-api', name: 'payments-api', path: `${HOME}/payments-api` },
];

export const LAUNCHERS = [
  { id: 'claude', provider: 'claude', label: 'Claude Code', permissionMode: 'default' },
  { id: 'codex', provider: 'codex', label: 'Codex', permissionMode: 'default' },
  { id: 'hermes', provider: 'hermes', label: 'Hermes', permissionMode: 'default' },
  { id: 'opencode', provider: 'opencode', label: 'OpenCode', permissionMode: 'default' },
];

/** One thing said or done in a demo chat: a prompt, or a part of the answer after it. */
type Step = { ask: string } | { part: ChatPart };

let ids = 0;
const next = (prefix: string) => `${prefix}-${++ids}`;
const user = (text: string): Step => ({ ask: text });
const say = (text: string): Step => ({ part: { kind: 'text', id: next('part'), text } });
const think = (text: string): Step => ({ part: { kind: 'thought', id: next('part'), text } });
const tool = (kind: ToolAction, title: string, status = 'completed', row: Partial<ToolCall['row']> = {}): Step => ({
  part: {
    kind: 'tool',
    id: next('part'),
    tool: { toolCallId: next('tool'), kind, title, status, row: { action: kind, target: title, ...row } },
  },
});
const file = (kind: ToolAction, path: string, name: string): Step => tool(kind, path, 'completed', { target: name, path });

type Pickers = Pick<ChatView['head'], 'model' | 'effort'>;

const gpt = { id: 'model', current: 'gpt-5.5', choices: [{ value: 'gpt-5.5', label: 'GPT-5.5' }] };

const PICKERS: Record<string, Pickers> = {
  claude: {
    model: {
      id: 'model',
      current: 'opus',
      choices: [
        { value: 'opus', label: 'Opus 5.5' },
        { value: 'sonnet', label: 'Sonnet 5' },
      ],
    },
    effort: {
      id: 'effort',
      current: 'high',
      choices: [
        { value: 'medium', label: 'Medium' },
        { value: 'high', label: 'High' },
      ],
    },
  },
  codex: {
    model: gpt,
    effort: {
      id: 'reasoning_effort',
      current: 'medium',
      choices: [
        { value: 'low', label: 'Low' },
        { value: 'medium', label: 'Medium' },
        { value: 'high', label: 'High' },
      ],
    },
  },
  hermes: { model: gpt },
};

export type DemoChat = {
  agentId: string;
  /** What the host's core named the agent. */
  name: string;
  provider: string;
  title: string;
  project: string;
  running: boolean;
  asleep?: boolean;
  permissionMode: string;
  history: Step[];
  /** A permission the agent is waiting on, shown on the phone as a card to answer. */
  asking?: { title: string; command: string };
  /** What it left when its last turn ended. */
  handoff?: { answer: string; files: string[]; recording?: number };
};

export const CHATS: DemoChat[] = [
  {
    agentId: 'chat-dark-mode',
    name: 'Otter',
    provider: 'claude',
    title: 'Add dark mode to settings',
    project: 'p-shop',
    running: true,
    permissionMode: 'default',
    asking: { title: 'Run the settings tests', command: 'pnpm test src/settings' },
    history: [
      user('Add a dark mode switch to the settings page. It should remember the choice.'),
      think('The theme lives in ThemeProvider. I need a toggle in Settings and a saved preference.'),
      file('read', 'src/settings/SettingsPage.tsx', 'SettingsPage.tsx'),
      file('read', 'src/theme/ThemeProvider.tsx', 'ThemeProvider.tsx'),
      say('I added a **Dark mode** switch under Appearance. It saves the choice, so the page opens the same way next time.'),
      file('edit', 'src/settings/SettingsPage.tsx', 'SettingsPage.tsx'),
      file('edit', 'src/theme/ThemeProvider.tsx', 'ThemeProvider.tsx'),
      say('Now I want to run the settings tests to make sure nothing else broke.'),
    ],
  },
  {
    agentId: 'chat-login',
    name: 'Maple',
    provider: 'codex',
    title: 'Fix the login form on small screens',
    project: 'p-shop',
    running: true,
    permissionMode: 'bypass',
    history: [
      user('On phones the login button is cut off. Fix the layout and check it in the browser.'),
      think('The form uses a fixed width. Switching it to fill the screen should fix it.'),
      tool('search', 'Find the login form styles'),
      say('The form had a fixed width of 480px, so it overflowed on phones. I changed it to fill the screen with some padding.'),
      file('edit', 'src/auth/LoginForm.css', 'LoginForm.css'),
      tool('fetch', 'Open the login page at phone size'),
      say('The button fits now on a 375px wide screen. Running the tests before I commit.'),
      user('Great, commit it when the tests pass.'),
      tool('execute', 'pnpm test src/auth', 'in_progress'),
    ],
  },
  {
    agentId: 'chat-login-check',
    name: 'Wren',
    provider: 'claude',
    title: 'Check the login form at every phone size',
    project: 'p-shop',
    running: true,
    permissionMode: 'bypass',
    history: [
      user('Open the login page at 320, 375 and 430 wide and tell me whether anything is cut off.'),
      tool('fetch', 'Open the login page at 320px'),
      say('At 320px the button fits, but the "Forgot password" link wraps under it. Checking 375 and 430 next.'),
      tool('fetch', 'Open the login page at 375px', 'in_progress'),
    ],
  },
  {
    agentId: 'chat-release',
    name: 'Birch',
    provider: 'hermes',
    title: 'Write the release notes',
    project: 'p-site',
    running: false,
    permissionMode: 'default',
    history: [
      user('Write release notes for this week from the merged pull requests.'),
      tool('execute', 'gh pr list --state merged'),
      say(
        'Here is a draft:\n\n- **Faster checkout** on slow connections\n- **Dark mode** in settings\n- Fixed the login button on small phones',
      ),
    ],
  },
  {
    agentId: 'chat-refunds',
    name: 'Flint',
    provider: 'claude',
    title: 'Add refunds to the payments API',
    project: 'p-api',
    running: false,
    asleep: true,
    permissionMode: 'default',
    history: [user('Add a refunds endpoint with tests.'), say('Added `POST /refunds` with validation and six tests. All of them pass.')],
    handoff: {
      answer: 'Added `POST /refunds` with validation and six tests. All of them pass.',
      files: ['src/refunds/route.ts', 'src/refunds/validate.ts', 'test/refunds.test.ts'],
      recording: 14,
    },
  },
];

/** Questions agents put to the person, as the host's questions call lists them. */
export const QUESTIONS = [
  {
    id: 'q-release',
    agentId: 'chat-release',
    name: 'Birch',
    cli: 'hermes',
    project: `${HOME}/marketing-site`,
    asked: 'Should the notes call out the new prices, or leave them for the blog post?',
    text: 'Should the notes call out the new prices, or leave them for the blog post?',
    choices: [
      { label: 'Call them out', detail: 'one line under Faster checkout' },
      { label: 'Leave them', detail: 'the blog post covers them' },
    ],
    blocked: true,
    assumption: null,
    secret: false,
    at: Date.now() - 120_000,
    state: 'open',
    viaNote: null,
    secretNote: null,
    answer: null,
    ticket: 'SITE-41',
  },
  {
    id: 'q-widths',
    agentId: 'chat-login-check',
    name: 'Wren',
    cli: 'claude',
    project: `${HOME}/acme-shop`,
    asked: 'Which phone widths should I check?',
    text: 'Which phone widths should I check?',
    choices: [],
    blocked: false,
    assumption: '320, 375 and 430 points cover the phones we support',
    secret: false,
    at: Date.now() - 60_000,
    state: 'assumed',
    viaNote: null,
    secretNote: null,
    answer: null,
  },
];

const card = (fields: Record<string, unknown>) => ({
  details: '',
  owner: null,
  ownerName: null,
  ownerCli: null,
  team: null,
  teamName: null,
  parent: null,
  dependsOn: [],
  waitsOn: [],
  createdBy: null,
  createdByName: 'You',
  result: null,
  ticket: null,
  auto: null,
  at: Date.now() - 86_400_000,
  updatedAt: Date.now() - 3_600_000,
  ...fields,
});

const jira = (key: string, source: string) => ({ key, source, provider: 'jira' });

/** Each project's board, as the host's boards call lists them. */
export const BOARDS = [
  {
    project: `${HOME}/acme-shop`,
    settings: {
      auto: false,
      slots: 3,
      launcher: null,
      model: null,
      finish: 'merge',
      check: null,
      statuses: { progress: 'In Progress', done: 'Done' },
    },
    running: 0,
    autoNote: null,
    cards: [
      card({
        id: 1,
        title: 'Dark mode in settings',
        status: 'progress',
        owner: 'chat-dark-mode',
        ownerName: 'Otter',
        ownerCli: 'claude',
        team: 'chat-dark-mode',
        teamName: 'Otter',
        ticket: jira('SHOP-12', 'Story'),
        updatedAt: Date.now() - 600_000,
      }),
      card({
        id: 2,
        title: 'Fix the login form on small phones',
        status: 'progress',
        owner: 'chat-login',
        ownerName: 'Maple',
        ownerCli: 'codex',
        team: 'chat-login',
        teamName: 'Maple',
        ticket: jira('SHOP-14', 'Bug'),
        updatedAt: Date.now() - 300_000,
      }),
      card({
        id: 3,
        title: 'Check the login form at every phone size',
        status: 'progress',
        owner: 'chat-login-check',
        ownerName: 'Wren',
        ownerCli: 'claude',
        team: 'chat-login',
        teamName: 'Maple',
        parent: 2,
        createdBy: 'chat-login',
        createdByName: 'Maple',
        updatedAt: Date.now() - 200_000,
      }),
      card({
        id: 4,
        title: 'Remember the theme on every device',
        status: 'open',
        dependsOn: [1],
        waitsOn: [1],
        ticket: jira('SHOP-15', 'Story'),
      }),
      card({
        id: 5,
        title: 'Move checkout totals into the order service',
        status: 'done',
        owner: 'chat-gone',
        ownerName: 'Comet',
        ownerCli: 'claude',
        team: 'chat-gone',
        teamName: 'Comet',
        ticket: jira('SHOP-9', 'Story'),
        updatedAt: Date.now() - 10_800_000,
      }),
    ],
  },
  {
    project: `${HOME}/marketing-site`,
    settings: {
      auto: false,
      slots: 3,
      launcher: null,
      model: null,
      finish: 'merge',
      check: null,
      statuses: { progress: 'In Progress', done: 'Done' },
    },
    running: 0,
    autoNote: null,
    cards: [
      card({
        id: 1,
        title: 'Release notes for this week',
        status: 'progress',
        owner: 'chat-release',
        ownerName: 'Birch',
        ownerCli: 'hermes',
        team: 'chat-release',
        teamName: 'Birch',
        ticket: jira('SITE-41', 'Story'),
      }),
      card({ id: 2, title: 'Blog post on the new prices', status: 'open' }),
    ],
  },
];

/** Saved chats the host lists as recent, with how many hours ago each was last written to. */
export const RECENT = [
  { id: 'claude:r1', provider: 'claude', title: 'Move checkout totals into the order service', project: 'p-shop', hours: 3 },
  { id: 'codex:r2', provider: 'codex', title: 'Why does the pricing page shift on load', project: 'p-site', hours: 26 },
  { id: 'claude:r3', provider: 'claude', title: 'Retry webhooks with backoff', project: 'p-api', hours: 74 },
];

export const SESSIONS = [
  { id: 11, project: 'p-shop', task: { label: 'dev', command: 'pnpm dev', cwd: `${HOME}/acme-shop`, project: 'p-shop' } },
  { id: 12, project: 'p-shop' },
  { id: 13, project: 'p-api', task: { label: 'api', command: 'pnpm start', cwd: `${HOME}/payments-api`, project: 'p-api' } },
];

/* A real chat's history says when each part of it happened, which is how a finished turn knows
   how long it worked. The demo's steps came nine seconds apart. */
const STEP_MS = 9_000;

/** An answer's last run of prose, which stays in view once the turn folds its work away. */
function replyStart(parts: ChatPart[]): number {
  let from = parts.length;
  while (from > 0 && (parts[from - 1].kind === 'text' || parts[from - 1].kind === 'content')) from -= 1;
  return from;
}

/** The demo chat's rows, as the host's core folds them: a row per prompt and per answer, finished turns folded. */
export function chatRows(chat: DemoChat): ChatMessage[] {
  const start = Date.now() - chat.history.length * STEP_MS;
  const rows: ChatMessage[] = [];
  let asked = start;
  chat.history.forEach((step, index) => {
    const at = start + index * STEP_MS;
    if ('ask' in step) {
      const id = next('m');
      asked = at;
      rows.push({ id, role: 'user', parts: [{ kind: 'text', id: next('part'), text: step.ask }], sentAt: at, meta: { from: id, at } });
      return;
    }
    const part = step.part.kind === 'tool' ? { ...step.part, startedAt: at, endedAt: at + 2_000 } : step.part;
    const last = rows.at(-1);
    if (last?.role === 'assistant') last.parts.push(part);
    else rows.push({ id: next('m'), role: 'assistant', parts: [part] });
    const answer = rows.at(-1) as ChatMessage;
    const finished = !(chat.running && index === chat.history.length - 1) && !('part' in (chat.history[index + 1] ?? { ask: '' }));
    if (!finished) return;
    const took = at + STEP_MS - asked;
    answer.endedAt = at + STEP_MS;
    answer.meta = { from: answer.id, at: answer.endedAt, took };
    const from = replyStart(answer.parts);
    const work = answer.parts.slice(0, from);
    if (from === answer.parts.length || !work.length) return;
    const calls = work.filter((done) => done.kind === 'tool').length;
    answer.fold = { id: answer.id, last: answer.id, from, calls, took, thought: calls === 0 };
  });
  return rows;
}

/** How the demo chat stands, as the head of the view a host sends. */
export function chatHead(chat: DemoChat): ChatView['head'] {
  const asking = permissionPayload(chat);
  return {
    sessionId: `session-${chat.agentId}`,
    connection: 'ready',
    running: chat.running,
    turned: true,
    permissionMode: chat.permissionMode,
    commands: [],
    capabilities: {},
    setup: {},
    ...PICKERS[chat.provider],
    permissions: asking ? [asking] : [],
    tasks: [],
    runningSubagents: [],
    queued: [],
    earlierLost: false,
  };
}

export function permissionPayload(chat: DemoChat): PermissionAsk | undefined {
  if (!chat.asking) return undefined;
  return {
    requestId: `ask-${chat.agentId}`,
    sessionId: `session-${chat.agentId}`,
    toolCall: {
      toolCallId: `ask-tool-${chat.agentId}`,
      title: chat.asking.command,
      kind: 'execute',
      row: { action: 'execute', target: chat.asking.command },
    },
    options: [
      { optionId: 'allow', name: 'Allow', kind: 'allow_once' },
      { optionId: 'always', name: 'Always allow', kind: 'allow_always' },
      { optionId: 'reject', name: 'Deny', kind: 'reject_once' },
    ],
  };
}

const MINUTE = 60_000;
const logged = (ago: number, from: string, to: string, kind: string, outcome: string) => ({
  at: Date.now() - ago * MINUTE,
  from,
  kind,
  to,
  outcome,
});

/** acme-shop's log of messages between its agents, newest first. */
export const MESSAGES = [
  logged(1, 'chat-dark-mode', 'chat-refunds', 'message', 'steered'),
  logged(3, 'chat-login-check', 'chat-dark-mode', 'question', 'waiting'),
  logged(6, 'chat-login', 'chat-refunds', 'answer', 'delivered'),
  logged(9, 'chat-refunds', 'chat-dark-mode', 'report', 'newTurn'),
  logged(14, 'chat-login', 'chat-refunds', 'answer', 'waiting'),
  logged(22, 'chat-dark-mode', 'chat-login-check', 'message', 'dropped'),
  logged(31, 'chat-dark-mode', 'chat-login-check', 'message', 'waiting'),
  logged(52, 'chat-dark-mode', 'chat-login', 'message', 'refused'),
];
