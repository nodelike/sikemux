/*
 * The Mac the phone showcase pretends to be paired with: its projects, agents,
 * terminals and chats, written to read well in a screenshot.
 */

import type {
  AgentLetter,
  AgentNotice,
  ChatGoal,
  ChatMessage,
  ChatPart,
  ChatView,
  ChiefAway,
  GoalMarker,
  InLine,
  PermissionAsk,
  PermissionMode,
  PersonCall,
  ToolAction,
  ToolCall,
  ToolDetail,
  TurnState,
} from '@shared/wire.generated';
import ADAPTER_HUNT from './recorded/adapter-hunt.json';
import LOGIN_FIX from './recorded/login-fix.json';
import SIGNUP_CHECK from './recorded/signup-check.json';

export const CORE = 'showcase0000000000000000000000000000000000000000000000000000core';
export const HOST = { name: 'MacBook Pro', model: 'MacBook Pro' };
export const PANE_PICTURE = '/showcase/pane.jpg';

const HOME = '/Users/edon/code';

export const PROJECTS = [
  {
    id: 'p-shop',
    name: 'acme-shop',
    path: `${HOME}/acme-shop`,
    codeHost: { host: 'github', repo: 'edon/acme-shop', ciName: 'Actions', sections: ['pulls', 'actions', 'issues', 'releases'] },
  },
  {
    id: 'p-site',
    name: 'marketing-site',
    path: `${HOME}/marketing-site`,
    git: { branch: 'release-notes', ahead: 2, behind: 0, dirty: false },
  },
  { id: 'p-api', name: 'payments-api', path: `${HOME}/payments-api`, chief: 'chat-team' },
];

export const LAUNCHERS = [
  { id: 'claude', provider: 'claude', label: 'Claude Code', permissionMode: 'default' },
  { id: 'codex', provider: 'codex', label: 'Codex', permissionMode: 'default' },
  { id: 'hermes', provider: 'hermes', label: 'Hermes', permissionMode: 'default' },
  { id: 'opencode', provider: 'opencode', label: 'OpenCode', permissionMode: 'default' },
];

/** One thing said or done in a demo chat: a prompt, or a part of the answer after it. */
type Step = { ask: string } | { part: ChatPart } | { letters: AgentLetter[] };

let ids = 0;
const next = (prefix: string) => `${prefix}-${++ids}`;
const user = (text: string): Step => ({ ask: text });
const say = (text: string): Step => ({ part: { kind: 'text', id: next('part'), text } });
const think = (text: string): Step => ({ part: { kind: 'thought', id: next('part'), text } });
const tool = (kind: ToolAction, title: string, status: ToolCall['status'] = 'completed', row: Partial<ToolCall['row']> = {}): Step => ({
  part: {
    kind: 'tool',
    id: next('part'),
    tool: { toolCallId: next('tool'), kind, title, status, row: { action: kind, target: title, ...row } },
  },
});
const letters = (...sent: AgentLetter[]): Step => ({ letters: sent });
const goal = (marker: GoalMarker): Step => ({
  part: { kind: 'agent_notice', id: next('part'), notice: { severity: 'info', title: marker.name ?? 'Goal', goal: marker } },
});
/** A line the core puts in the chief's chat: an Overrule, or the week's count. */
const notice = (said: Pick<AgentNotice, 'overruled' | 'week'>): Step => ({
  part: {
    kind: 'agent_notice',
    id: next('part'),
    notice: { severity: 'info', title: said.week ? "The week's count" : 'Overruled', ...said },
  },
});
const DAY = 86_400_000;
/** This week's Monday at 09:00, when the chief's count of the week before came. */
const WEEK_START = (() => {
  const monday = new Date();
  monday.setDate(monday.getDate() - ((monday.getDay() + 6) % 7));
  monday.setHours(9, 0, 0, 0);
  return monday.getTime();
})();
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

type DemoChat = {
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
  /** The goal it works on now. */
  goal?: ChatGoal;
  /** Left out of the host's agent list, so only its own scene opens it. */
  unlisted?: boolean;
  /**
   * A chat a real core folded, as it sent it, with what each call changed or printed: the strips need the runs the
   * core counts, which a written history does not have.
   */
  recorded?: Recorded;
};

export type Recorded = Pick<ChatView, 'head' | 'rows' | 'subagents'> & { details: Record<string, ToolDetail> };

const recorded = (agentId: string, name: string, project: string, view: unknown): DemoChat => {
  const chat = view as Recorded;
  return {
    agentId,
    name,
    provider: 'claude',
    title: chat.rows.find((row) => row.role === 'user')?.parts.find((part) => part.kind === 'text')?.text ?? name,
    project,
    running: chat.head.running,
    permissionMode: chat.head.permissionMode,
    history: [],
    recorded: chat,
  };
};

/** Chats opened only by their own scenes, so the device's lists stay as the site's pictures show them. */
export const RECORDED: DemoChat[] = [
  recorded('chat-login-fix', 'Sango', 'p-shop', LOGIN_FIX),
  recorded('chat-signup-check', 'Kuro', 'p-shop', SIGNUP_CHECK),
  recorded('chat-adapter-hunt', 'Ivy', 'p-shop', ADAPTER_HUNT),
];

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
  {
    agentId: 'chat-notes',
    name: 'Kuro',
    provider: 'claude',
    title: 'Release notes for 2.4',
    project: 'p-site',
    running: false,
    unlisted: true,
    permissionMode: 'bypass',
    goal: { project: 'p-site', card: 41, name: 'Ship 2.4', state: 'moving', steps: ['done', 'moving', 'waiting'] },
    history: [
      goal({ card: 41, name: 'Ship 2.4', state: 'moving', steps: ['done', 'moving', 'waiting'] }),
      user('Write release notes for this week from the merged pull requests.'),
      user('Keep them short, and link each one.'),
      tool('execute', 'gh pr list --state merged'),
      say(
        [
          '## Release 2.4',
          '### What is new',
          '- **Faster checkout** on slow connections ([#212](https://github.com/edon/acme-shop/pull/212))',
          '- **Dark mode** in settings, from [SHOP-88](https://edon.atlassian.net/browse/SHOP-88)',
          '### Shipping it',
          '1. Merge the notes branch',
          '2. Tag `v2.4.0` and let CI publish',
          '```ts:src/lib/release.ts\nexport const version = "2.4.0";\nexport const notes = "notes/2.4.md";\n```',
        ].join('\n\n'),
      ),
      goal({ card: 41, name: 'Ship 2.4', state: 'moving', steps: ['done', 'done', 'waiting'], moment: 'step', took: 1_560_000 }),
      goal({ card: 41, name: 'Ship 2.4', state: 'waiting', steps: ['done', 'done', 'waiting'], moment: 'stopped', took: 2_400_000 }),
    ],
  },
  {
    agentId: 'chat-team',
    name: 'Chief',
    provider: 'claude',
    title: 'Ledger import',
    project: 'p-api',
    running: false,
    permissionMode: 'bypass',
    history: [
      letters({
        kind: 'message',
        name: 'Otter',
        agentId: 'chat-dark-mode',
        cli: 'claude',
        text: 'Take PROJ-12: move the ledger import onto the new amounts table. Ask me if the data is unclear.',
      }),
      say('On it. Two questions first.'),
      letters(
        {
          kind: 'answer',
          name: 'Chief',
          agentId: 'chat-team',
          cli: 'claude',
          question: 'Which timezone should the ledger dates use?',
          text: 'UTC',
        },
        {
          kind: 'answer',
          name: 'Chief',
          agentId: 'chat-team',
          cli: 'claude',
          question: 'Store amounts in cents or as decimals?',
          text: 'Cents, and convert at the API edge',
        },
      ),
      say('Dates go in as UTC and amounts as cents. The import runs clean.'),
      letters({
        kind: 'trigger',
        name: 'CI on main',
        cli: 'claude',
        text: 'Read the failed run, fix it on a branch and open a pull request.',
        trigger: { title: 'CI on main', prompt: 'Read the failed run, fix it on a branch and open a pull request.' },
      }),
      notice({ week: { from: WEEK_START - 7 * DAY, to: WEEK_START, decided: 212, overruled: 3, answered: 14, asPicked: 12 } }),
      notice({
        overruled: { project: `${HOME}/payments-api`, decision: 7, text: 'Refunds post to the ledger as negative amounts', rule: 3 },
      }),
      notice({
        overruled: { project: `${HOME}/payments-api`, decision: 9, text: 'Keep the old ledger table for a week', undone: true },
      }),
    ],
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

const AGO = (ms: number) => Date.now() - ms;

/** Opened with `?line`, two agents wait on the host: Maple in line for a command, Wren for a thing Otter holds. */
const LINE_SCENE = typeof location !== 'undefined' && new URLSearchParams(location.search).has('line');

/** How each demo chat's turn stands, as the core says it on its row and its chat's last line. */
export const TURNS: Record<string, TurnState> = {
  'chat-dark-mode': { state: 'working', since: AGO(80_000), on: 'Adding the dark mode switch' },
  'chat-login': {
    state: 'waiting',
    since: AGO(240_000),
    wait: { kind: 'ci', what: 'CI', startedAt: AGO(240_000), expectedMs: 420_000 },
  },
  'chat-login-check': { state: 'working', since: AGO(40_000) },
  'chat-release': {
    state: 'needsYou',
    since: AGO(120_000),
    call: { id: 'call-release', question: 'Should the notes call out the new prices?', waitingSince: AGO(120_000), count: 1 },
  },
  'chat-refunds': { state: 'stopped', since: AGO(3_600_000), stopped: { left: 'Two refund tests still fail' } },
  'chat-notes': {
    state: 'done',
    since: AGO(600_000),
    done: { changes: 'committed', checks: 'pass', next: [{ step: 'push', primary: true }] },
  },
  'chat-team': {
    state: 'needsYou',
    since: AGO(300_000),
    call: { id: 'call-ledger', question: 'Ship the ledger import to production?', waitingSince: AGO(300_000), count: 1 },
  },
  ...(LINE_SCENE
    ? {
        'chat-login': {
          state: 'waiting',
          since: AGO(60_000),
          wait: { kind: 'line', what: 'pnpm test', startedAt: AGO(60_000), expectedMs: 240_000, place: 2 },
        },
        'chat-login-check': {
          state: 'waiting',
          since: AGO(720_000),
          wait: { kind: 'inUse', what: 'Stripe webhook listener', startedAt: AGO(720_000), by: 'Otter' },
        },
      }
    : {}),
};

/** Where the `?line` scenes' agents stand on the host: one in line for a command, one waiting for a thing. */
export const LINES: Record<string, InLine> = LINE_SCENE
  ? {
      'chat-login': { what: 'pnpm test', place: 2, expectedMs: 240_000 },
      'chat-login-check': { what: 'Stripe webhook listener', place: 1, by: 'Otter' },
    }
  : {};

/** The calls the demo agents put to the person, open and answered. */
export const CALLS: PersonCall[] = [
  {
    id: 'call-release',
    project: `${HOME}/marketing-site`,
    kind: 'taste',
    asker: { agentId: 'chat-release', name: 'Birch', cli: 'hermes' },
    question: 'Should the notes call out the new prices?',
    context: 'The blog post on Thursday covers them in full.',
    options: [
      { id: 'out', label: 'Call them out', detail: 'one line under Faster checkout' },
      { id: 'leave', label: 'Leave them', detail: 'the blog post covers them' },
    ],
    pick: 'out',
    against: 'the notes go out two days before the post.',
    canHandBack: true,
    raisedAt: AGO(120_000),
    state: 'open',
  },
  {
    id: 'call-ledger',
    project: `${HOME}/payments-api`,
    kind: 'production',
    asker: { agentId: 'chat-team', name: 'Chief', cli: 'claude', chief: true },
    question: 'Ship the ledger import to production?',
    options: [
      { id: 'all', label: 'Ship it to everyone' },
      { id: 'half', label: 'Ship to half first' },
      { id: 'wait', label: 'Not yet' },
    ],
    pick: 'all',
    brief: {
      tried: 'It ran on staging against last month’s ledger.',
      shows: 'Every total matched to the cent.',
      cannotUndo: 'Imported rows stay once written.',
    },
    action: 'vercel deploy --prod',
    canHandBack: false,
    raisedAt: AGO(300_000),
    state: 'open',
  },
  {
    id: 'call-cache',
    project: `${HOME}/acme-shop`,
    kind: 'taste',
    asker: { agentId: 'term-cache', name: 'Wren', cli: 'codex' },
    question: 'Keep product pages cached for an hour?',
    options: [
      { id: 'hour', label: 'An hour', detail: 'prices change at most daily' },
      { id: 'five', label: 'Five minutes' },
    ],
    pick: 'hour',
    against: 'a price fix would take up to an hour to show.',
    canHandBack: true,
    raisedAt: AGO(540_000),
    state: 'open',
  },
  {
    id: 'call-font',
    project: `${HOME}/acme-shop`,
    kind: 'money',
    asker: { agentId: 'chat-dark-mode', name: 'Otter', cli: 'claude' },
    question: 'Buy the display font, $40?',
    options: [
      { id: 'buy', label: 'Buy it' },
      { id: 'free', label: 'Use a free font' },
    ],
    pick: 'buy',
    canHandBack: true,
    raisedAt: AGO(7_200_000),
    state: 'answered',
    answer: { option: 'buy', by: { kind: 'device', id: 'phone' }, at: AGO(7_000_000), waitedMs: 200_000 },
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
    if ('letters' in step) {
      rows.push({ id: next('m'), role: 'user', parts: [], letters: step.letters, sentAt: at });
      return;
    }
    if ('ask' in step) {
      const id = next('m');
      asked = at;
      rows.push({ id, role: 'user', promptId: id, parts: [{ kind: 'text', id: next('part'), text: step.ask }], sentAt: at });
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
    const shown = answer.parts.flatMap((_, at) => (at >= from ? [at] : []));
    answer.fold = { id: answer.id, last: answer.id, from, shown, calls, took, thought: calls === 0 };
  });
  return rows;
}

/** A recorded chat's view with its times moved up to now, so a running call counts from a moment ago. */
export function recordedView(chat: Recorded): Pick<ChatView, 'head' | 'rows' | 'subagents'> {
  const times = JSON.stringify(chat).match(/"(?:startedAt|endedAt|sentAt|at)":\d+/g) ?? [];
  const latest = Math.max(...times.map((time) => Number(time.split(':')[1])));
  const shift = Date.now() - 20_000 - latest;
  const moved = JSON.parse(JSON.stringify(chat), (key, value) =>
    ['startedAt', 'endedAt', 'sentAt', 'at'].includes(key) && typeof value === 'number' ? value + shift : value,
  ) as Recorded;
  return { head: moved.head, rows: moved.rows, subagents: moved.subagents };
}

/** How the demo chat stands, as the head of the view a host sends. */
/** What happened while the person was away from the payments-api chief, as the core counts it. */
const AWAY: ChiefAway = {
  since: Date.now() - 7_380_000,
  back: Date.now() - 60_000,
  calls: ['call-ledger'],
  decided: [
    {
      project: `${HOME}/payments-api`,
      decision: 11,
      text: 'Refunds over $500 wait for a second approval before they post',
      why: '3 of 5 support tickets asked for it',
      by: { agentId: 'chat-refunds', name: 'Flint' },
      at: Date.now() - 3_600_000,
    },
    {
      project: `${HOME}/payments-api`,
      decision: 12,
      text: 'Webhooks retry three times before giving up',
      by: { agentId: 'chat-refunds', name: 'Flint' },
      at: Date.now() - 3_000_000,
      overruled: true,
    },
  ],
  landed: [{ agentId: 'chat-refunds', name: 'Flint', title: 'Add refunds to the payments API', at: Date.now() - 2_400_000 }],
};

export function chatHead(chat: DemoChat): ChatView['head'] {
  const asking = permissionPayload(chat);
  return {
    sessionId: `session-${chat.agentId}`,
    connection: 'ready',
    running: chat.running,
    turned: true,
    permissionMode: chat.permissionMode as PermissionMode,
    commands: [],
    capabilities: { editing: true },
    setup: {},
    ...PICKERS[chat.provider],
    permissions: asking ? [asking] : [],
    tasks: [],
    runningSubagents: [],
    queued: [],
    earlierLost: false,
    goal: chat.goal,
    turn: TURNS[chat.agentId],
    chief: chat.agentId === 'chat-team' || undefined,
    chiefHears: chat.project === 'p-api' && chat.agentId !== 'chat-team' ? 'chat-team' : undefined,
    away: chat.agentId === 'chat-team' ? AWAY : undefined,
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

/** The files `+` ranks in acme-shop, as its host's file list holds them. */
export const PROJECT_FILES = [
  'src/auth/login.tsx',
  'src/auth/session.ts',
  'src/auth/LoginForm.test.tsx',
  'src/checkout/Cart.tsx',
  'src/theme/dark.css',
  'docs/login.md',
];

/** The open issues and pull requests of acme-shop's repository. */
export const TRACKED = [
  {
    kind: 'issue',
    number: 214,
    title: 'Login form loses the email after a failed attempt',
    url: 'https://github.com/edon/acme-shop/issues/214',
  },
  { kind: 'issue', number: 209, title: 'Dark mode flashes white on load', url: 'https://github.com/edon/acme-shop/issues/209' },
  { kind: 'pull', number: 221, title: 'feat(auth): remember the last email', url: 'https://github.com/edon/acme-shop/pull/221' },
];
