/*
 * Stands in for `@sikemux/native`, the Rust client, when the app runs in the
 * showcase: one Mac is already paired and answers from the demo world instead
 * of the network.
 */
import { ChatAttachment, CoreEvent, MobileError } from '../app/test/mocks/sikemux-native';
import { CHATS, CORE, HOST, LAUNCHERS, PANE_PICTURE, PROJECTS, RECENT, SESSIONS, chatEvents, permissionPayload, setupOf } from './world';

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
      provider: chat.provider,
      title: chat.title,
      cwd: projectPath(chat.project),
      state: 'ready',
      running: chat.running,
      pendingPermissions: chat.asking ? [`ask-${chat.agentId}`] : [],
      launcher: chat.provider,
      permissionMode: chat.permissionMode,
      asleep: chat.asleep ?? false,
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
  };
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
    const events = chatEvents(chat);
    return ChatAttachment.Live.new({
      sessionId: `session-${chat.agentId}`,
      capabilitiesJson: '{}',
      setupJson: JSON.stringify(setupOf(chat)),
      permissionMode: chat.permissionMode,
      running: chat.running,
      turned: false,
      replayJson: JSON.stringify(events),
      mark: { feed: chat.agentId, seq: BigInt(events.length) },
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
  async call(method: string): Promise<string> {
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
