/*
 * Stands in for `@sikemux/native`, the Rust client, when the app runs in the
 * showcase: one Mac is already paired and answers from the demo world instead
 * of the network.
 */
import { BuildChannel, ChatAttachment, ChatState, CoreEvent, MobileError, SessionKind } from '../app/test/mocks/sikemux-native';
import { CHATS, CORE, HOST, LAUNCHERS, PANE_PICTURE, PROJECTS, SESSIONS, chatEvents, permissionPayload, setupOf } from './world';

export { BuildChannel, ChatAttachment, ChatState, CoreEvent, MobileError, SessionKind };

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
  output(session: bigint, bytes: ArrayBuffer): void;
  events(events: unknown[]): void;
  closed(): void;
};

const projectPath = (id: string) => PROJECTS.find((project) => project.id === id)?.path ?? '/';

function view() {
  return {
    workspace: {
      projects: PROJECTS,
      launchers: LAUNCHERS,
      palette: new Map<string, string>(),
      backdrop: { texture: true, image: 'pane' },
    },
    sessions: SESSIONS.map((session) => ({
      id: session.id,
      kind: session.task ? SessionKind.Task : SessionKind.Terminal,
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
      state: ChatState.Ready,
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
              requestJson: JSON.stringify(request),
              at: BigInt(Date.now() - 90_000),
            },
          ]
        : [];
    }),
  };
}

class Connection {
  private open = true;

  constructor(listener: Listener) {
    setTimeout(() => listener.events([CoreEvent.View.new({ view: view() })]), 0);
  }

  isOpen() {
    return this.open;
  }

  close() {
    this.open = false;
  }

  async host() {
    return { ...HOST, version: '0.4.2', channel: BuildChannel.Stable };
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
  async cancel() {}
  async answerPermission() {}
  async setChatConfig(_agentId: string, _configId: string, value: string) {
    return value;
  }
  async startChat() {
    return CHATS[0].agentId;
  }
  async attach() {
    return { replay: new ArrayBuffer(0), alternateScreen: false, exited: false };
  }
  async write() {}
  async resize() {}
  ack() {}
  async unpair() {}
  async saveBackdrop() {
    return undefined;
  }
}

export class Device {
  static async create() {
    return new Device();
  }

  id() {
    return 'a3f9c27e51d04b8e9c6f2a7d18e3b5c40f9e7a2d6b1c8e5f3a0d9c7b6e4f2a1d';
  }

  async connect(_core: string, listener: Listener) {
    return new Connection(listener);
  }

  async pair() {
    return CORE;
  }

  async close() {}
}

export const newDeviceKey = (): ArrayBuffer => new Uint8Array(32).buffer;

export const parsePairingLink = (): undefined => undefined;
