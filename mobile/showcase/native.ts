/*
 * Stands in for `@sikemux/native`, the Rust client, when the app runs in the
 * showcase: one Mac is already paired and answers from the demo world instead
 * of the network.
 */
import AGENT_CATALOG from '../../src/test/agentCatalog.generated.json';
import type { ChatMessage } from '../../src/chat/types';
import { ChatAttachment, CoreEvent, MobileError } from '../app/test/mocks/sikemux-native';
import {
  BOARDS,
  CHATS,
  CORE,
  HOST,
  LAUNCHERS,
  MESSAGES,
  PANE_PICTURE,
  PROJECTS,
  QUESTIONS,
  RECENT,
  SESSIONS,
  chatHead,
  chatRows,
  permissionPayload,
} from './world';

export { ChatAttachment, CoreEvent, MobileError };

const files = ((globalThis as { sikemuxTestFiles?: Map<string, string> }).sikemuxTestFiles ??= new Map());
files.set(
  'file:///document/paired-devices.json',
  JSON.stringify([
    {
      core: CORE,
      access: 'full',
      pairedAt: Date.now() - 86_400_000 * 6,
      name: HOST.name,
      model: HOST.model,
      channel: 'stable',
      backdrop: { texture: true, image: { id: 'pane', uri: PANE_PICTURE } },
      lastSeen: Date.now(),
    },
  ]),
);

type Listener = {
  events(events: unknown[]): void;
  closed(): void;
};

const projectPath = (id: string) => PROJECTS.find((project) => project.id === id)?.path ?? '/';

/** The words a core shows for each group. */
const GROUP_LABELS = { blocked: 'Needs input', working: 'Working', done: 'Done', idle: 'Idle' } as const;

/** How the core shows each demo chat, in the order it lists them. */
function shown() {
  return CHATS.map((chat) => {
    const asks = chat.asking || QUESTIONS.some((question) => question.agentId === chat.agentId && question.blocked);
    const group = asks ? 'blocked' : chat.running ? 'working' : 'idle';
    return {
      agentId: chat.agentId,
      cli: chat.provider,
      title: chat.title,
      status: group,
      group,
      label: GROUP_LABELS[group],
      unread: false,
      subagents: 0,
      tasks: 0,
      busy: group !== 'idle',
      background: false,
      activeAt: Date.now(),
    };
  }).sort((a, b) => Object.keys(GROUP_LABELS).indexOf(a.group) - Object.keys(GROUP_LABELS).indexOf(b.group));
}

/** The host's inbox as its core would rank the demo world. */
function inbox() {
  const by = (chat: (typeof CHATS)[number]) => ({
    agentId: chat.agentId,
    name: chat.name,
    cli: chat.provider,
    project: chat.project,
    title: chat.title,
    feed: chat.agentId,
    recordings: [],
  });
  const chat = (agentId: string) => CHATS.find((known) => known.agentId === agentId) ?? CHATS[0];
  const needs = [
    ...CHATS.filter((known) => known.asking).map((known) => ({
      ...by(known),
      id: `permission:${known.agentId}`,
      tier: 'needs',
      kind: 'permission',
      what: 'Needs your permission',
      detail: known.asking?.command,
      at: Date.now() - 90_000,
      requestId: `ask-${known.agentId}`,
    })),
    ...QUESTIONS.map((question) => ({
      ...by(chat(question.agentId)),
      id: `question:${question.id}`,
      tier: 'needs',
      kind: question.blocked ? 'question' : 'assumption',
      what: question.blocked ? 'Asks you' : `Went on assuming ${question.assumption}`,
      detail: question.text,
      at: question.at,
      questionId: question.id,
    })),
  ];
  const listed = new Set(needs.map((item) => item.agentId));
  const working = CHATS.filter((known) => known.running && !listed.has(known.agentId));
  const finished = CHATS.filter((known) => known.handoff).map((known) => ({
    ...by(known),
    id: `agent:${known.agentId}`,
    tier: 'review',
    kind: 'changes',
    what: 'Finished, with changes to review',
    at: Date.now() - 1_800_000,
    handoff: { answer: known.handoff?.answer, files: known.handoff?.files, edited: known.handoff?.files.length },
    recordings: known.handoff?.recording
      ? [{ path: '/Users/edon/Movies/Sikemux/refunds.mov', seconds: known.handoff.recording, at: Date.now() - 1_900_000 }]
      : [],
  }));
  return {
    items: [
      ...needs,
      ...finished,
      ...working.map((known) => ({
        ...by(known),
        id: `agent:${known.agentId}`,
        tier: 'progress',
        kind: 'working',
        what: 'Working',
        at: Date.now() - 300_000,
      })),
    ],
    working: working.length,
    away: false,
    agentMessages: {},
    remoteAgents: [],
    agents: shown(),
  };
}

function view() {
  return {
    workspace: {
      projects: PROJECTS,
      launchers: LAUNCHERS,
      palette: {},
      backdrop: { texture: true, image: 'pane' },
    },
    sessions: SESSIONS.map((session) => ({
      id: session.id,
      kind: session.task ? 'task' : 'terminal',
      running: true,
      cols: 120,
      rows: 34,
      project: session.project,
      task: session.task,
      killed: false,
    })),
    chats: CHATS.map((chat) => ({
      agentId: chat.agentId,
      name: chat.name,
      provider: chat.provider,
      title: chat.title,
      cwd: projectPath(chat.project),
      state: 'ready',
      running: chat.running,
      pendingPermissions: chat.asking ? [`ask-${chat.agentId}`] : [],
      launcher: chat.provider,
      permissionMode: chat.permissionMode,
      asleep: chat.asleep ?? false,
      group: QUESTIONS.some((question) => question.agentId === chat.agentId && question.blocked) ? 'blocked' : undefined,
    })),
    attentions: CHATS.flatMap((chat) => {
      const request = permissionPayload(chat);
      return request
        ? [
            {
              id: `ask-${chat.agentId}`,
              agentId: chat.agentId,
              provider: chat.provider,
              cwd: projectPath(chat.project),
              request,
              at: Date.now() - 90_000,
              choices: { allowOnce: 'allow', reject: 'reject' },
            },
          ]
        : [];
    }),
    recent: RECENT.map((chat) => ({
      id: chat.id,
      provider: chat.provider,
      title: chat.title,
      project: chat.project,
      cwd: projectPath(chat.project),
      activeAt: Date.now() - chat.hours * 3_600_000,
    })),
    inbox: inbox(),
  };
}

/** Gives each text its whole markdown tree, as a host sends it, read by the core's own parser. */
async function withTrees(rows: ChatMessage[]): Promise<ChatMessage[]> {
  const parts = rows.flatMap((row) => row.parts.flatMap((part) => (part.kind === 'text' || part.kind === 'thought' ? [part] : [])));
  const response = await fetch('/showcase/markdown', { method: 'POST', body: JSON.stringify(parts.map((part) => part.text)) });
  const trees = (await response.json()) as unknown[][];
  parts.forEach((part, index) => {
    part.md = { blocks: trees[index], n: part.text.length };
  });
  return rows;
}

class Connection {
  private open = true;

  constructor(listener: Listener) {
    setTimeout(() => listener.events([CoreEvent.View.new({ viewJson: JSON.stringify(view()) })]), 0);
  }

  isOpen() {
    return this.open;
  }

  close() {
    this.open = false;
  }

  async host() {
    return JSON.stringify({ ...HOST, version: '0.4.2', channel: 'stable' });
  }

  async wakeChat() {}

  async attachChat(agentId: string) {
    const chat = CHATS.find((known) => known.agentId === agentId);
    if (!chat) return ChatAttachment.Missing.new({});
    const rows = await withTrees(chatRows(chat));
    return ChatAttachment.Live.new({
      viewJson: JSON.stringify({ head: chatHead(chat), rows }),
      mark: { feed: chat.agentId, seq: 0n },
    });
  }

  async detachChat() {}
  async prompt() {}
  async attachFile(_agentId: string, name: string) {
    return `/Users/demo/Library/Caches/com.nodelike.sikemux/pasted/${name}`;
  }
  async cancel() {}
  async answerPermission() {}
  async setChatConfig(_agentId: string, _configId: string, value: string) {
    return value;
  }
  async startChat() {
    return CHATS[0].agentId;
  }
  async resumeChat() {
    return CHATS[0].agentId;
  }
  async unpair() {}
  async saveBackdrop() {
    return undefined;
  }
  async call(method: string, params: string): Promise<string> {
    if (method === 'boards') return JSON.stringify(BOARDS);
    if (method === 'board') {
      const { project } = JSON.parse(params) as { project: string };
      const board = BOARDS.find((known) => known.project === project);
      return JSON.stringify(board ?? { ...BOARDS[0], project, cards: [] });
    }
    if (method === 'questions') return JSON.stringify(QUESTIONS);
    if (method === 'agentCatalog') return JSON.stringify(AGENT_CATALOG);
    if (method === 'agentMessages') {
      const { project } = JSON.parse(params) as { project: string };
      return JSON.stringify({ project, entries: project === BOARDS[0].project ? MESSAGES : [] });
    }
    if (method === 'digest') return JSON.stringify({ away: false, finished: [], failed: [], waiting: [] });
    if (method === 'chatToolDetail') return '{}';
    throw new Error(`the showcase host does not know ${method}`);
  }
  async callToFile(method: string): Promise<string> {
    throw new Error(`the showcase host does not know ${method}`);
  }
}

const PHONE_ID = 'a3f9c27e51d04b8e9c6f2a7d18e3b5c40f9e7a2d6b1c8e5f3a0d9c7b6e4f2a1d';

export class DeviceIdentity {
  id() {
    return PHONE_ID;
  }

  signLive() {
    return '';
  }

  signRegistration() {
    return '';
  }

  signPush() {
    return '';
  }
}

export class Device {
  static async create() {
    return new Device();
  }

  id() {
    return PHONE_ID;
  }

  async connect(_core: string, listener: Listener) {
    return new Connection(listener);
  }

  async close() {}
}

export const newDeviceKey = (): ArrayBuffer => new Uint8Array(32).buffer;
