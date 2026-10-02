/*
 * The Mac the phone showcase pretends to be paired with: its projects, agents,
 * terminals and chats, written to read well in a screenshot.
 */

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

type Update = Record<string, unknown>;

let ids = 0;
const next = (prefix: string) => `${prefix}-${++ids}`;
const user = (text: string): Update => ({ sessionUpdate: 'user_message_chunk', messageId: next('m'), content: { type: 'text', text } });
const say = (text: string): Update => ({ sessionUpdate: 'agent_message_chunk', messageId: next('m'), content: { type: 'text', text } });
const think = (text: string): Update => ({ sessionUpdate: 'agent_thought_chunk', messageId: next('m'), content: { type: 'text', text } });
const tool = (kind: string, title: string, status = 'completed', extra: Update = {}): Update => ({
  sessionUpdate: 'tool_call',
  toolCallId: next('tool'),
  kind,
  title,
  status,
  ...extra,
});

const MODELS: Record<string, unknown[]> = {
  claude: [
    {
      type: 'select',
      id: 'model',
      name: 'Model',
      currentValue: 'opus',
      options: [
        { value: 'opus', name: 'Opus 5.5' },
        { value: 'sonnet', name: 'Sonnet 5' },
      ],
    },
    {
      type: 'select',
      id: 'effort',
      name: 'Effort',
      currentValue: 'high',
      options: [
        { value: 'medium', name: 'Medium' },
        { value: 'high', name: 'High' },
      ],
    },
  ],
  codex: [
    { type: 'select', id: 'model', name: 'Model', currentValue: 'gpt-5.5', options: [{ value: 'gpt-5.5', name: 'GPT-5.5' }] },
    {
      type: 'select',
      id: 'reasoning_effort',
      name: 'Reasoning',
      currentValue: 'medium',
      options: [
        { value: 'low', name: 'Low' },
        { value: 'medium', name: 'Medium' },
        { value: 'high', name: 'High' },
      ],
    },
  ],
  hermes: [{ type: 'select', id: 'model', name: 'Model', currentValue: 'gpt-5.5', options: [{ value: 'gpt-5.5', name: 'GPT-5.5' }] }],
};

export type DemoChat = {
  agentId: string;
  provider: string;
  title: string;
  project: string;
  running: boolean;
  asleep?: boolean;
  permissionMode: string;
  history: Update[];
  /** A permission the agent is waiting on, shown on the phone as a card to answer. */
  asking?: { title: string; command: string };
};

export const CHATS: DemoChat[] = [
  {
    agentId: 'chat-dark-mode',
    provider: 'claude',
    title: 'Add dark mode to settings',
    project: 'p-shop',
    running: true,
    permissionMode: 'default',
    asking: { title: 'Run the settings tests', command: 'pnpm test src/settings' },
    history: [
      user('Add a dark mode switch to the settings page. It should remember the choice.'),
      think('The theme lives in ThemeProvider. I need a toggle in Settings and a saved preference.'),
      tool('read', 'src/settings/SettingsPage.tsx'),
      tool('read', 'src/theme/ThemeProvider.tsx'),
      say('I added a **Dark mode** switch under Appearance. It saves the choice, so the page opens the same way next time.'),
      tool('edit', 'src/settings/SettingsPage.tsx'),
      tool('edit', 'src/theme/ThemeProvider.tsx'),
      say('Now I want to run the settings tests to make sure nothing else broke.'),
    ],
  },
  {
    agentId: 'chat-login',
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
      tool('edit', 'src/auth/LoginForm.css'),
      tool('fetch', 'Open the login page at phone size'),
      say('The button fits now on a 375px wide screen. Running the tests before I commit.'),
      user('Great, commit it when the tests pass.'),
      tool('execute', 'pnpm test src/auth', 'in_progress'),
    ],
  },
  {
    agentId: 'chat-release',
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
    provider: 'claude',
    title: 'Add refunds to the payments API',
    project: 'p-api',
    running: false,
    asleep: true,
    permissionMode: 'default',
    history: [user('Add a refunds endpoint with tests.'), say('Added `POST /refunds` with validation and six tests. All of them pass.')],
  },
];

export const SESSIONS = [
  { id: 11n, project: 'p-shop', task: { label: 'dev', command: 'pnpm dev', cwd: `${HOME}/acme-shop`, project: 'p-shop' } },
  { id: 12n, project: 'p-shop' },
  { id: 13n, project: 'p-api', task: { label: 'api', command: 'pnpm start', cwd: `${HOME}/payments-api`, project: 'p-api' } },
];

export function chatEvents(chat: DemoChat): { kind: string; payload: Record<string, unknown> }[] {
  const sessionId = `session-${chat.agentId}`;
  const events: { kind: string; payload: Record<string, unknown> }[] = [
    { kind: 'ready', payload: { capabilities: {}, setup: { configOptions: MODELS[chat.provider] ?? [] } } },
    { kind: 'status', payload: { state: 'ready' } },
    ...chat.history.map((update) => ({ kind: 'session_update', payload: { sessionId, update } })),
  ];
  if (chat.running) events.push({ kind: 'turn_started', payload: {} });
  const asking = permissionPayload(chat);
  if (asking) events.push({ kind: 'permission_request', payload: asking });
  return events;
}

export function permissionPayload(chat: DemoChat): Record<string, unknown> | undefined {
  if (!chat.asking) return undefined;
  return {
    requestId: `ask-${chat.agentId}`,
    sessionId: `session-${chat.agentId}`,
    toolCall: { toolCallId: `ask-tool-${chat.agentId}`, title: chat.asking.command, kind: 'execute' },
    options: [
      { optionId: 'allow', name: 'Allow', kind: 'allow_once' },
      { optionId: 'always', name: 'Always allow', kind: 'allow_always' },
      { optionId: 'reject', name: 'Deny', kind: 'reject_once' },
    ],
  };
}

export function setupOf(chat: DemoChat) {
  return { configOptions: MODELS[chat.provider] ?? [] };
}
