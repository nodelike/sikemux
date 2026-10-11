/*
 * The core's protocol as the phone reads it. What it shows of a host arrives as the core's own JSON,
 * so a field the core adds reaches the phone with an over-the-air update. These types follow
 * `DeviceView` and `HostInfo` in `src-tauri/crates/sikemux-wire/src/protocol.rs`.
 */

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
};

export type ChatState = 'starting' | 'ready' | 'stopped';

export type ChatInfo = {
  agentId: string;
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
  startedBy?: string;
  launcher?: string;
  permissionMode: string;
  model?: string;
  effort?: string;
  asleep: boolean;
  /** It finished or asked for something nobody has looked at yet. */
  unread: boolean;
  activeAt?: number;
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

/** Everything the phone shows of one host, as it last sent it. */
export type Snapshot = {
  workspace: Workspace;
  sessions: SessionInfo[];
  chats: ChatInfo[];
  attentions: Attention[];
  /** Newest first. */
  recent: RecentInfo[];
};

/** Kept with the paired host, so it is a name the core sends as is. */
export type BuildChannel = 'dev' | 'nightly' | 'stable';

export type HostInfo = { name: string; model: string; version: string; channel: BuildChannel };

/** The core's JSON with its nulls left out, so an absent value reads as undefined, like any optional field. */
function readCore<T>(json: string): T {
  return JSON.parse(json, (_key, value) => (value === null ? undefined : value)) as T;
}

export const readView = (json: string) => readCore<Snapshot>(json);

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
  };
}
export const readHost = (json: string) => readCore<HostInfo>(json);

/** A chat event as the Mac app's chat code reads it. */
export type CoreChatEvent = { kind: string; payload: Record<string, unknown> };
