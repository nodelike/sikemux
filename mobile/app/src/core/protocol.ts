/*
 * The core's protocol as the phone reads it. What it shows of a host arrives as the core's own JSON,
 * so a field the core adds reaches the phone with an over-the-air update. These types follow
 * `DeviceView` and `HostInfo` in `src-tauri/crates/sikemux-wire/src/protocol.rs`.
 */

import type { PublishedBuddies } from '@mac/buddies/published';

export type ProjectInfo = { id: string; name: string; path: string };

export type LauncherInfo = {
  id: string;
  provider: string;
  label: string;
  permissionMode: string;
  /** The `configOptions` the provider's last session offered; absent before one has started. */
  configOptions?: unknown;
  status?: string;
};

/** What the host draws behind its panes: the moving grain, or the picture `saveBackdrop` fetches by `image`. */
export type Backdrop = { texture: boolean; image?: string };

export type Workspace = {
  projects: ProjectInfo[];
  launchers: LauncherInfo[];
  /** The host's theme colours by name; empty until its app publishes them. */
  palette: Record<string, string>;
  backdrop: Backdrop;
};

export type TaskInfo = {
  executionId: string;
  terminalKey: string;
  taskId: string;
  label: string;
  project: string;
  source: string;
  command: string;
  cwd: string;
  agentId?: string;
};

export type SessionInfo = {
  id: number;
  kind: 'terminal' | 'task';
  pid?: number;
  running: boolean;
  cols: number;
  rows: number;
  attached: number;
  project?: string;
  paneId?: string;
  agentId?: string;
  /** What the core calls an agent terminal's agent, such as Otter. */
  agentName?: string;
  agentType?: string;
  taskExecutionId?: string;
  agentState?: string;
  task?: TaskInfo;
  exit?: { code?: number; signal?: string };
  killed: boolean;
  startedBy?: string;
  /** What the host's app calls the agent terminal. */
  title?: string;
  /** The agent finished or asked for something nobody has looked at yet. */
  unread: boolean;
  activeAt?: number;
  /** An agent terminal's group, as the core ranks it. */
  group?: AgentGroup;
  /** An agent terminal's place among the open agents, in the order they want the person: 0 first. */
  attentionRank?: number;
};

export type ChatState = 'starting' | 'ready' | 'stopped';

/** The group the core puts an agent in, most in need of the person first. */
export type AgentGroup = 'blocked' | 'working' | 'done' | 'idle';

export type ChatInfo = {
  agentId: string;
  /** What the core calls the agent, such as Otter. */
  name?: string;
  provider: string;
  title?: string;
  cwd: string;
  sessionId?: string;
  state: ChatState;
  running: boolean;
  pendingPermissions: string[];
  /** Subagents still running for it. */
  subagents: number;
  /** Background shells and monitors still running for it; hosts from before it was sent leave it out. */
  tasks?: number;
  /** The attempt its agent is retrying a failed request on, 0 when it did not say. */
  reconnecting?: number | null;
  startedBy?: string;
  launcher?: string;
  permissionMode: string;
  model?: string;
  effort?: string;
  asleep: boolean;
  /** It finished or asked for something nobody has looked at yet. */
  unread: boolean;
  activeAt?: number;
  /** The core's group for it, which counts a question it waits on the person to answer. */
  group?: AgentGroup;
  /** Its place among the open agents, chats and agent terminals together, in the order they want the person: 0 first. */
  attentionRank?: number;
};

/** The options a permission request comes down to, and what the agent wants to do. */
export type PermissionChoices = {
  allowOnce?: string;
  allowAlways?: string;
  reject?: string;
  /** The shell command it wants to run, when it is one. */
  command?: string;
  title?: string;
};

/** An agent waiting on a person to answer a permission request. */
export type Attention = {
  id: string;
  kind: string;
  agentId: string;
  provider: string;
  cwd: string;
  /** The agent's own request. */
  request?: Record<string, unknown>;
  /** Milliseconds since the Unix epoch. */
  at: number;
  choices: PermissionChoices;
};

/** A saved chat the host's app lists as recent, which `resumeChat` takes up again. */
export type RecentInfo = {
  id: string;
  provider: string;
  title: string;
  /** The id of the host's project it ran in. */
  project: string;
  cwd: string;
  /** Milliseconds since the Unix epoch. */
  activeAt: number;
};

/** Waits on the person, then changes to review, then finished work, then work under way. */
export type InboxTier = 'needs' | 'review' | 'finished' | 'progress';

export type InboxKind =
  'permission' | 'question' | 'input' | 'held' | 'assumption' | 'pullRequest' | 'changes' | 'failed' | 'finished' | 'working';

/** A browser recording an agent made, kept on the host. */
export type Recording = { path: string; seconds: number; at: number; feed?: string; turn?: number };

/** What an agent left when its turn ended. */
export type Handoff = { answer?: string; result?: string; files: string[]; edited: number };

/** One thing in the host's inbox: who, what they need or did, and where to open it. `what` and `detail` are ready to show. */
export type InboxItem = {
  id: string;
  tier: InboxTier;
  kind: InboxKind;
  agentId?: string;
  name?: string;
  cli?: string;
  /** The project's id, or the folder the agent works in. */
  project?: string;
  title?: string;
  what: string;
  detail?: string;
  at: number;
  /** The agent's terminal; none for a chat. */
  session?: number;
  feed?: string;
  turn?: number;
  card?: { project: string; id: number; title: string };
  /** Answer with `answerPermission`. */
  requestId?: string;
  /** Answer with the `answerQuestion` call. */
  questionId?: string;
  url?: string;
  handoff?: Handoff;
  recordings: Recording[];
  /** The linked host the agent runs on, when it is not this one. */
  host?: string;
};

export type RemoteStatus = AgentGroup | 'stopped' | 'starting' | 'asleep';

/** A worker an agent here started on a linked host. */
export type RemoteAgent = {
  agentId: string;
  name: string;
  /** What this host calls the host it runs on. */
  host: string;
  hostId: string;
  cli: string;
  status: RemoteStatus;
  /** The agent here that started it. */
  lead?: string;
  project: string;
  /** Its host answers now. */
  reachable: boolean;
};

/** The host's inbox, ranked by the core. */
export type Inbox = {
  items: InboxItem[];
  working: number;
  /** When each project, by its path, last logged a message between its agents. */
  agentMessages?: Record<string, number>;
  /** Workers agents here started on linked hosts; hosts from before links leave it out. */
  remoteAgents?: RemoteAgent[];
  /** The characters the host's app draws for its agents; absent while buddies are off there. */
  buddies?: PublishedBuddies;
};

/** Everything the phone shows of one host, as it last sent it. */
export type Snapshot = {
  workspace: Workspace;
  sessions: SessionInfo[];
  chats: ChatInfo[];
  attentions: Attention[];
  /** Newest first. */
  recent: RecentInfo[];
  inbox: Inbox;
};

export type DeviceAccess = 'full' | 'watch';

/** A device waiting for someone to let it pair with the host. */
export type PendingPairing = { id: string; deviceId: string; name: string; platform: string; expiresAt: number };

/** A device paired with the host, and what it may do there. */
export type PairedClient = { id: string; name: string; platform: string; access: DeviceAccess; pairedAt: number; lastSeen?: number };

/** Another host on the account linked with this one, so agents on either can start agents on the other. */
export type LinkedHost = {
  /** Its key. */
  id: string;
  name: string;
  platform: string;
  linkedAt: number;
  lastSeen?: number;
  connected: boolean;
  /** Agents of shared teams on it. */
  agents: number;
};

/** What the `pairing` call answers, and `answerPairing` and `changeDevice` after them. */
export type Pairing = {
  pending: PendingPairing[];
  devices: PairedClient[];
  links?: LinkedHost[];
  /** Other hosts asking to link with this one. */
  pendingLinks?: PendingPairing[];
};

/** Kept with the paired host, so it is a name the core sends as is. */
export type BuildChannel = 'dev' | 'nightly' | 'stable';

export type HostInfo = { name: string; model: string; version: string; channel: BuildChannel };

/** The core's JSON with its nulls left out, so an absent value reads as undefined, like any optional field. */
function readCore<T>(json: string): T {
  return JSON.parse(json, (_key, value) => (value === null ? undefined : value)) as T;
}

/** Hosts from before the inbox send none, which reads as an empty one. */
export function readView(json: string): Snapshot {
  const view = readCore<Omit<Snapshot, 'inbox'> & { inbox?: Inbox }>(json);
  return { ...view, inbox: view.inbox ?? { items: [], working: 0 } };
}

function same(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== 'object' || typeof b !== 'object' || a === null || b === null) return false;
  if (Array.isArray(a) !== Array.isArray(b)) return false;
  const keys = Object.keys(a);
  if (keys.length !== Object.keys(b).length) return false;
  return keys.every((key) => same((a as Record<string, unknown>)[key], (b as Record<string, unknown>)[key]));
}

/** The items of `next`, each the previous one with its key when nothing in it changed. */
function keepItems<T>(previous: T[], next: T[], key: (item: T) => string | number): T[] {
  const before = new Map(previous.map((item) => [key(item), item]));
  const kept = next.map((item) => {
    const old = before.get(key(item));
    return old !== undefined && same(old, item) ? old : item;
  });
  return kept.length === previous.length && kept.every((item, index) => item === previous[index]) ? previous : kept;
}

/**
 * A host sends its whole view whenever anything in it changes, several times a second while an agent works. Each
 * part of the new view that equals the last one's is the last one's own object, so a screen or row that shows only
 * unchanged parts is not drawn again.
 */
export function keepUnchanged(previous: Snapshot | undefined, next: Snapshot): Snapshot {
  if (!previous) return next;
  return {
    workspace: same(previous.workspace, next.workspace) ? previous.workspace : next.workspace,
    sessions: keepItems(previous.sessions, next.sessions, (session) => session.id),
    chats: keepItems(previous.chats, next.chats, (chat) => chat.agentId),
    attentions: keepItems(previous.attentions, next.attentions, (attention) => attention.id),
    recent: keepItems(previous.recent, next.recent, (chat) => chat.id),
    inbox: keepInbox(previous.inbox, next.inbox),
  };
}

function keepInbox(previous: Inbox, next: Inbox): Inbox {
  const items = keepItems(previous.items, next.items, (item) => item.id);
  const agentMessages = same(previous.agentMessages, next.agentMessages) ? previous.agentMessages : next.agentMessages;
  const remoteAgents = next.remoteAgents && keepItems(previous.remoteAgents ?? [], next.remoteAgents, (agent) => agent.agentId);
  const buddies = same(previous.buddies, next.buddies) ? previous.buddies : next.buddies;
  return items === previous.items &&
    previous.working === next.working &&
    agentMessages === previous.agentMessages &&
    remoteAgents === previous.remoteAgents &&
    buddies === previous.buddies
    ? previous
    : { items, working: next.working, agentMessages, remoteAgents, buddies };
}
export const readHost = (json: string) => readCore<HostInfo>(json);
export const readPairing = (json: string) => readCore<Pairing>(json);

/** A chat event as the Mac app's chat code reads it. */
export type CoreChatEvent = { kind: string; payload: Record<string, unknown> };
