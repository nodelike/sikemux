import { ChatAttachment, MobileError, type ChatMark, type ConnectionLike } from '@sikemux/native';
import { File } from 'expo-file-system';

import { recordOf } from '@mac/chat/acpEvents';
import { chatReducer, initialChatState } from '@mac/chat/reducer';
import type { AcpPermissionRequest, ChatAction, ChatMessage, ChatState } from '@mac/chat/types';

import type { ChatInfo, CoreChatEvent } from '@/core/protocol';
import type { ChatDelivery } from '@/devices/hub';
import { actions, parsed } from './chatEvents';
import { earlierMessages, PAGE_TURNS, withEarlier } from './earlier';
import { Folds } from './folds';

export type Attached = 'attaching' | 'live' | 'missing';

/** A message this phone wrote that the host has not taken yet. */
export type Unsent = { state: 'sending' | 'failed'; problem?: string };

/** The host's own limit on a file sent to a chat. */
export const MAX_ATTACHMENT_BYTES = 10 * 1024 * 1024;
export const MAX_ATTACHMENTS = 10;

/** A photo or file picked on this phone for the next message. */
export type Attachment = {
  id: string;
  kind: 'image' | 'file';
  name: string;
  mime: string;
  /** Where it is on this phone. */
  uri: string;
  size: number;
  /** Where the host keeps it, once it is there. */
  path?: string;
  upload?: 'sending' | 'failed';
  problem?: string;
};

/** A message waiting behind the running turn. */
export type Held = { id: string; text: string; attachments: Attachment[] };

/** The messages that go out together once the turn ends; a slash command only works as the whole prompt, so it goes alone. */
function nextBatch(queued: Held[]): Held[] {
  const isCommand = (held: Held) => held.text.startsWith('/');
  if (!queued.length || isCommand(queued[0])) return queued.slice(0, 1);
  const command = queued.findIndex(isCommand);
  return command === -1 ? queued : queued.slice(0, command);
}

function combined(messages: Held[]): { text: string; attachments: Attachment[] } {
  return {
    text: messages
      .map((held) => held.text)
      .filter(Boolean)
      .join('\n\n'),
    attachments: messages.flatMap((held) => held.attachments),
  };
}

export type ChatSnapshot = {
  /** The chat as the host tells it, with what this phone sent added in place. */
  agent: ChatState;
  /** The permission requests still waiting on the host. */
  permissions: AcpPermissionRequest[];
  /** Messages rebuilt from a replay or an earlier page, which carry no times. */
  replayed: ReadonlySet<string>;
  attached: Attached;
  /** Why the chat could not be opened, in the host's words. */
  problem: string | null;
  /** Something this phone asked for that did not happen, such as a stop or a setting. */
  notice: string | null;
  /** By message id. */
  unsent: ReadonlyMap<string, Unsent>;
  queued: readonly Held[];
  /** Picked for the next message. */
  attachments: readonly Attachment[];
  /** What this phone sent, by the path the host keeps it at. */
  sentFiles: ReadonlyMap<string, Attachment>;
  /** Permission requests whose answer is on its way. */
  answering: ReadonlySet<string>;
  /** Background tasks this phone asked the host to stop. */
  stopping: ReadonlySet<string>;
  /** How the chat asks before acting, as this phone last set it or the host last said; null before either. */
  permissionMode: string | null;
  changingMode: boolean;
  hasEarlier: boolean;
  earlier: 'idle' | 'loading' | 'failed';
  /** When this phone saw the running turn begin. */
  turnSince: number;
};

type Options = {
  /** Hands over this host's chat events as they arrive. */
  listen: (take: (deliveries: ChatDelivery[]) => void) => () => void;
  /** Runs once before the next frame is drawn; returns how to call that off. */
  frame?: (run: () => void) => () => void;
  now?: () => number;
  /** The contents of a file on this phone. */
  read?: (uri: string) => Promise<ArrayBuffer>;
};

/** `steer` puts it into the running turn rather than after it. */
type Outgoing = { key: number; messageId: string; text: string; paths: string[]; steer: boolean } & Unsent;

/** Where the host's history of this run of the chat goes on before what the phone holds. */
type Cursor = { feed: string; before: bigint };

/** Earlier pages a fallen-back replay left a gap above, kept until the turns between are fetched. */
type Bridge = { until: bigint; messages: ChatMessage[]; cursor: Cursor | null };

const TOO_LONG = 'This chat is longer than the host keeps for the phone. Open it on the host to carry on.';

/**
 * A permission request that just arrived is shown before the host's list of waiting requests,
 * which comes on its own schedule, has caught up with it.
 */
const PERMISSION_GRACE_MS = 2000;

function reason(error: unknown): string {
  if (MobileError.Refused.instanceOf(error) || MobileError.Connection.instanceOf(error)) return error.inner.message;
  return error instanceof Error ? error.message : String(error);
}

function readFile(uri: string): Promise<ArrayBuffer> {
  return new File(uri).arrayBuffer();
}

function animationFrame(run: () => void): () => void {
  const frame = requestAnimationFrame(run);
  return () => cancelAnimationFrame(frame);
}

/** Adds a message from this phone without saying a turn has begun, for one the host has not taken. */
function withMessage(state: ChatState, text: string, paths: string[]): ChatState {
  const next = chatReducer(state, { type: 'local_prompt', text, paths });
  return { ...next, running: state.running, suppressUserEcho: state.suppressUserEcho };
}

/**
 * One chat on one host, kept outside React so it outlives the screen: what the host said, where
 * its events got to, what this phone sent and what it still means to send.
 */
export class ChatSession {
  readonly folds = new Folds();
  private listeners = new Set<() => void>();
  private agent: ChatState = initialChatState;
  private replayed: ReadonlySet<string> = new Set();
  private attached: Attached = 'attaching';
  private problem: string | null = null;
  private notice: string | null = null;
  private outbox: Outgoing[] = [];
  private sent = 0;
  private queued: Held[] = [];
  private attachments: Attachment[] = [];
  private sentFiles: ReadonlyMap<string, Attachment> = new Map();
  private answering: ReadonlySet<string> = new Set();
  private stopping: ReadonlySet<string> = new Set();
  private hostMode: string | undefined;
  private mode: string | null = null;
  private changingMode = false;
  private draft = '';
  private turnSince: number;
  /** A turn this phone started is taken as running until the host says otherwise. */
  private optimistic = false;

  /** The last event the transcript holds; a reconnect asks only for what comes after it. */
  private mark: ChatMark | undefined;
  /** The last event taken, drawn or not yet. */
  private taken: bigint | undefined;
  private pending: ChatAction[] = [];
  private cancelFrame: (() => void) | undefined;

  private cursor: Cursor | null = null;
  private earlier: ChatSnapshot['earlier'] = 'idle';
  /** Where the replay on screen begins, to tell whether earlier pages still join on to it. */
  private replayStart: bigint | undefined;
  private earlierShown: ChatMessage[] = [];
  private bridge: Bridge | undefined;
  private pages = 0;

  /** What the host lists as waiting, or null before it has said. */
  private onHost: ReadonlySet<string> | null = null;
  private arrived = new Map<string, number>();
  private hostUp: boolean | undefined;

  private connection: ConnectionLike | undefined;
  private holders = 0;
  /** Counts attaches, so an answer that comes back after the chat was taken up again is dropped. */
  private run = 0;
  private stopRun: (() => void) | undefined;
  private snap: ChatSnapshot;

  constructor(
    readonly agentId: string,
    private readonly options: Options,
  ) {
    this.turnSince = this.now();
    this.snap = this.build();
  }

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  snapshot = () => this.snap;

  draftText = () => this.draft;

  setDraft = (text: string) => {
    if (text === this.draft) return;
    this.draft = text;
    this.notify();
  };

  /** Keeps the chat attached over this connection until the returned function is called. */
  hold = (connection: ConnectionLike | undefined) => {
    if (!connection) return () => {};
    this.holders += 1;
    if (this.connection !== connection) {
      this.stop();
      this.connection = connection;
      this.start();
    }
    return () => {
      this.holders -= 1;
      if (this.holders > 0 || this.connection !== connection) return;
      this.stop();
      this.connection = undefined;
    };
  };

  /**
   * What the host's own view says of this chat. A chat that comes back after it slept, stopped or
   * restarted is taken up again, and only the requests the host still waits on are shown.
   */
  hostSaw = (info: ChatInfo | undefined, waiting: string[] | undefined) => {
    const up = info !== undefined && info.state === 'ready' && !info.asleep;
    const cameBack = this.hostUp === false && up;
    this.hostUp = up;
    if (info && info.permissionMode !== this.hostMode) {
      this.hostMode = info.permissionMode;
      this.mode = null;
      this.emit();
    }
    const next = waiting ? new Set(waiting) : null;
    const same = next && this.onHost && next.size === this.onHost.size && [...next].every((id) => this.onHost?.has(id));
    if (!same && (next || this.onHost)) {
      this.onHost = next;
      this.emit();
    }
    if (cameBack && this.connection && this.attached !== 'attaching') this.restart();
  };

  /** Adds what was picked for the next message, leaving out what the host would refuse. */
  attach = (picked: Attachment[]) => {
    const tooBig = picked.filter((attachment) => attachment.size > MAX_ATTACHMENT_BYTES);
    const fits = picked.filter((attachment) => attachment.size <= MAX_ATTACHMENT_BYTES);
    const room = Math.max(0, MAX_ATTACHMENTS - this.attachments.length);
    this.attachments = [...this.attachments, ...fits.slice(0, room)];
    if (tooBig.length) {
      const names = tooBig.map((attachment) => attachment.name).join(', ');
      this.notice = `${names} ${tooBig.length === 1 ? 'is' : 'are'} over 10 MB, too big to send`;
    } else if (fits.length > room) {
      this.notice = `a message takes at most ${MAX_ATTACHMENTS} files`;
    } else {
      this.notice = null;
    }
    this.emit();
  };

  removeAttachment = (id: string) => {
    if (this.uploading()) return;
    this.attachments = this.attachments.filter((attachment) => attachment.id !== id);
    this.emit();
  };

  /**
   * Sends the message once every file picked for it is on the host. Answers whether it went out
   * or waits behind the turn; when a file did not get there, it stays in the composer to try again.
   */
  send = async (text: string): Promise<boolean> => {
    this.notice = null;
    let attachments: Attachment[] = [];
    if (this.attachments.length) {
      if (this.uploading()) return false;
      attachments = await this.upload();
      if (!attachments.length) return false;
    }
    if (this.busy()) {
      this.queued = [...this.queued, { id: `queued-${(this.sent += 1)}`, text, attachments }];
      this.emit();
      return true;
    }
    this.deliver(text, attachments);
    return true;
  };

  retrySend = (messageId: string) => {
    const entry = this.outbox.find((outgoing) => outgoing.messageId === messageId);
    if (!entry || entry.state !== 'failed') return;
    this.update(entry, { state: 'sending', problem: undefined });
    this.push(entry);
  };

  /** Puts waiting messages into the running turn now, as one, instead of after it. */
  steer = (ids: readonly string[]) => {
    const steered = this.queued.filter((held) => ids.includes(held.id));
    if (!steered.length) return;
    this.queued = this.queued.filter((held) => !ids.includes(held.id));
    const { text, attachments } = combined(steered);
    this.deliver(text, attachments, true);
  };

  dropQueued = (id: string) => {
    this.queued = this.queued.filter((held) => held.id !== id);
    this.emit();
  };

  /** Stops the running turn. Messages waiting behind it go back to the composer rather than out. */
  cancel = () => {
    if (this.queued.length) {
      const { text, attachments } = combined(this.queued);
      this.draft = [text, this.draft].filter(Boolean).join('\n\n');
      this.attachments = [...attachments, ...this.attachments];
      this.queued = [];
    }
    this.notice = null;
    this.emit();
    this.ask((connection) => connection.cancel(this.agentId), 'Could not stop the turn');
  };

  answer = (requestId: string, optionId: string | null) => {
    if (this.answering.has(requestId)) return;
    this.answering = new Set([...this.answering, requestId]);
    this.notice = null;
    this.emit();
    this.ask(
      (connection) => connection.answerPermission(this.agentId, requestId, optionId ?? undefined),
      'The answer did not reach the host',
      () => {
        this.agent = chatReducer(this.agent, { type: 'permission_cleared', requestId });
      },
    ).finally(() => {
      this.answering = new Set([...this.answering].filter((id) => id !== requestId));
      this.emit();
    });
  };

  stopTask = (taskId: string) => {
    if (this.stopping.has(taskId)) return;
    this.stopping = new Set([...this.stopping, taskId]);
    this.notice = null;
    this.emit();
    this.ask((connection) => connection.stopTask(this.agentId, taskId), 'Could not stop the task').finally(() => {
      this.stopping = new Set([...this.stopping].filter((id) => id !== taskId));
      this.emit();
    });
  };

  /** `bypass` runs without asking; `workspace-write` asks first. */
  setPermissionMode = (mode: string) => {
    if (this.changingMode) return;
    this.changingMode = true;
    this.notice = null;
    this.emit();
    this.ask(
      (connection) => connection.setPermissionMode(this.agentId, mode),
      'Could not change permissions',
      () => {
        this.mode = mode;
      },
    ).finally(() => {
      this.changingMode = false;
      this.emit();
    });
  };

  /** Ends the chat's agent on the host. Answers whether it did. */
  stopChat = async (): Promise<boolean> => {
    let stopped = false;
    await this.ask(
      (connection) => connection.stopChat(this.agentId),
      'Could not stop the chat',
      () => {
        stopped = true;
      },
    );
    return stopped;
  };

  setConfig = (configId: string, value: string) => {
    this.notice = null;
    this.emit();
    this.ask(
      (connection) => connection.setChatConfig(this.agentId, configId, value),
      'Could not change the setting',
      (json) => {
        const options = recordOf(JSON.parse(json))?.configOptions;
        if (options) this.agent = chatReducer(this.agent, { type: 'config', options });
      },
    );
  };

  /** Says what went wrong where the person is looking. */
  report = (text: string) => {
    this.notice = text;
    this.emit();
  };

  dismissNotice = () => {
    this.notice = null;
    this.emit();
  };

  /** Fetches the turns before the first message shown, unless that is already under way. */
  loadEarlier = () => {
    const connection = this.connection;
    const cursor = this.cursor;
    if (!cursor || !connection || this.earlier === 'loading') return;
    this.earlier = 'loading';
    this.emit();
    const run = this.run;
    connection
      .chatHistory(this.agentId, cursor.feed, cursor.before, PAGE_TURNS)
      .then((page) => {
        if (run !== this.run) return;
        this.pages += 1;
        const messages = earlierMessages(parsed(page.eventsJson), this.pages);
        this.showEarlier(messages);
        this.cursor = page.olderBefore === undefined ? null : { feed: cursor.feed, before: page.olderBefore };
        this.earlier = 'idle';
        this.crossBridge(page.olderBefore);
        this.emit();
        if (this.bridge) this.loadEarlier();
      })
      .catch((error: unknown) => {
        if (run !== this.run) return;
        this.bridge = undefined;
        if (MobileError.Refused.instanceOf(error)) {
          this.cursor = null;
          this.earlier = 'idle';
        } else {
          this.earlier = 'failed';
        }
        this.emit();
      });
  };

  /** Opens the chat again from the start, after it could not be opened. */
  retry = () => {
    this.mark = undefined;
    this.taken = undefined;
    if (this.connection) this.restart();
  };

  private now() {
    return (this.options.now ?? Date.now)();
  }

  private notify() {
    this.listeners.forEach((listener) => listener());
  }

  private emit() {
    this.snap = this.build();
    this.notify();
  }

  private build(): ChatSnapshot {
    const now = this.now();
    const onHost = this.onHost;
    const permissions = onHost
      ? this.agent.permissions.filter(
          (request) => onHost.has(request.requestId) || now - (this.arrived.get(request.requestId) ?? -Infinity) < PERMISSION_GRACE_MS,
        )
      : this.agent.permissions;
    return {
      agent: this.agent,
      permissions: permissions.length === this.agent.permissions.length ? this.agent.permissions : permissions,
      replayed: this.replayed,
      attached: this.attached,
      problem: this.problem,
      notice: this.notice,
      unsent: this.unsentView(),
      queued: this.queued,
      attachments: this.attachments,
      sentFiles: this.sentFiles,
      answering: this.answering,
      stopping: this.stopping,
      permissionMode: this.mode ?? this.hostMode ?? null,
      changingMode: this.changingMode,
      hasEarlier: this.cursor !== null,
      earlier: this.earlier,
      turnSince: this.turnSince,
    };
  }

  private unsentFrom: Outgoing[] | undefined;
  private unsentMap: ReadonlyMap<string, Unsent> = new Map();

  private unsentView(): ReadonlyMap<string, Unsent> {
    if (this.unsentFrom !== this.outbox) {
      this.unsentFrom = this.outbox;
      this.unsentMap = new Map(this.outbox.map(({ messageId, state, problem }) => [messageId, { state, problem }]));
    }
    return this.unsentMap;
  }

  private uploading() {
    return this.attachments.some((attachment) => attachment.upload === 'sending');
  }

  private patch(id: string, change: Partial<Attachment>) {
    this.attachments = this.attachments.map((attachment) => (attachment.id === id ? { ...attachment, ...change } : attachment));
    this.emit();
  }

  /** Puts each picked file the host does not have yet on it, one at a time. Answers them all once all got there. */
  private async upload(): Promise<Attachment[]> {
    const connection = this.connection;
    const waiting = this.attachments.filter((attachment) => !attachment.path);
    for (const attachment of waiting) this.patch(attachment.id, { upload: 'sending', problem: undefined });
    for (const attachment of waiting) {
      try {
        if (!connection?.isOpen()) throw new Error('the host is not connected');
        const bytes = await (this.options.read ?? readFile)(attachment.uri);
        const path = await connection.attachFile(this.agentId, attachment.name, attachment.mime, bytes);
        this.sentFiles = new Map([...this.sentFiles, [path, attachment]]);
        this.patch(attachment.id, { upload: undefined, path });
      } catch (error) {
        this.patch(attachment.id, { upload: 'failed', problem: reason(error) });
      }
    }
    if (this.attachments.some((attachment) => !attachment.path)) return [];
    const sent = this.attachments;
    this.attachments = [];
    return sent;
  }

  private busy() {
    return this.agent.running || this.outbox.some((outgoing) => outgoing.state === 'sending');
  }

  private apply(batch: ChatAction[]) {
    for (const action of batch) {
      const was = this.agent.running;
      this.agent = chatReducer(this.agent, action);
      if (action.type === 'turn_started' || action.type === 'turn_completed' || action.type === 'error') this.optimistic = false;
      if (!was && this.agent.running) this.turnSince = this.now();
    }
  }

  private take(deliveries: ChatDelivery[]) {
    for (const delivery of deliveries) {
      if (this.taken !== undefined && delivery.seq <= this.taken) continue;
      const taken = actions(JSON.parse(delivery.eventJson) as CoreChatEvent);
      for (const action of taken) {
        if (action.type !== 'permission_requested') continue;
        this.arrived.set(action.request.requestId, this.now());
        setTimeout(() => this.emit(), PERMISSION_GRACE_MS);
      }
      this.pending.push(...taken);
      this.taken = delivery.seq;
    }
    // A stream of tokens is drawn once a frame, not once an event.
    this.cancelFrame ??= (this.options.frame ?? animationFrame)(this.flush);
  }

  private flush = () => {
    if (this.applyPending()) this.drain();
  };

  /** Draws what was taken, and only then moves the mark past it. */
  private applyPending(): boolean {
    this.cancelFrame?.();
    this.cancelFrame = undefined;
    const advanced = this.mark && this.taken !== undefined && this.taken !== this.mark.seq;
    if (!this.pending.length && !advanced) return false;
    const batch = this.pending;
    this.pending = [];
    this.apply(batch);
    if (this.mark && this.taken !== undefined) this.mark = { ...this.mark, seq: this.taken };
    this.emit();
    return true;
  }

  private restart() {
    this.stop();
    this.start();
  }

  private stop() {
    this.run += 1;
    this.stopRun?.();
    this.stopRun = undefined;
    if (this.earlier === 'loading') this.earlier = 'idle';
  }

  private start() {
    const connection = this.connection;
    if (!connection) return;
    const run = (this.run += 1);
    const agentId = this.agentId;
    if (!this.mark || this.attached === 'missing') this.attached = 'attaching';
    this.problem = null;
    this.emit();
    let held: ChatDelivery[] | undefined = [];
    const off = this.options.listen((deliveries) => {
      if (run !== this.run) return;
      const mine = deliveries.filter((delivery) => delivery.agentId === agentId);
      if (!mine.length) return;
      // Until the attach answer says where the replay ends, events wait.
      if (held) held.push(...mine);
      else this.take(mine);
    });
    this.stopRun = () => {
      off();
      // What arrived but was not drawn yet goes into the transcript now, so the next attach picks up after it.
      this.applyPending();
      if (connection.isOpen()) connection.detachChat(agentId).catch(() => {});
    };

    // A chat the host put to sleep starts again first; one already running answers at once.
    connection
      .wakeChat(agentId)
      .then(() => connection.attachChat(agentId, this.mark))
      .then((attachment) => {
        if (run !== this.run) return;
        if (ChatAttachment.Live.instanceOf(attachment)) {
          this.replace(attachment.inner);
        } else if (ChatAttachment.Resumed.instanceOf(attachment)) {
          this.apply(parsed(attachment.inner.eventsJson));
          this.mark = attachment.inner.mark;
          this.taken = attachment.inner.mark.seq;
        } else {
          this.problem = ChatAttachment.Restart.instanceOf(attachment) ? TOO_LONG : null;
          this.attached = 'missing';
          this.emit();
          return;
        }
        this.hostUp = true;
        const waiting = held ?? [];
        held = undefined;
        this.attached = 'live';
        this.take(waiting);
        this.emit();
        this.drain();
      })
      .catch((error: unknown) => {
        if (run !== this.run) return;
        this.problem = reason(error);
        this.attached = 'missing';
        this.emit();
      });
  }

  private replace(live: {
    replayJson: string;
    capabilitiesJson: string;
    setupJson: string;
    running: boolean;
    mark: ChatMark;
    olderBefore?: bigint;
  }) {
    const rebuilt = parsed(live.replayJson).reduce(chatReducer, initialChatState);
    const replayed = new Set(rebuilt.messages.map((message) => message.id));
    const wasRunning = this.agent.running;
    let agent = chatReducer(rebuilt, {
      type: 'ready',
      capabilities: recordOf(JSON.parse(live.capabilitiesJson)) ?? {},
      setup: recordOf(JSON.parse(live.setupJson)) ?? {},
    });
    if (live.running) agent = chatReducer(agent, { type: 'turn_started' });
    if (live.running && !wasRunning) this.turnSince = this.now();

    // The replay has what the host took; what it did not take is put back after it.
    this.outbox = this.outbox.map((outgoing) => {
      agent = withMessage(agent, outgoing.text, outgoing.paths);
      return { ...outgoing, messageId: agent.messages[agent.messages.length - 1].id };
    });

    const sameFeed = this.mark?.feed === live.mark.feed;
    const before = this.earlierShown;
    const start = this.replayStart;
    const kept = { messages: before, cursor: this.cursor };
    this.earlierShown = [];
    this.bridge = undefined;
    this.cursor = live.olderBefore === undefined ? null : { feed: live.mark.feed, before: live.olderBefore };
    if (sameFeed && before.length && start !== undefined && live.olderBefore !== undefined) {
      if (live.olderBefore === start) {
        agent = withEarlier(agent, before);
        this.earlierShown = before;
        this.cursor = kept.cursor;
        before.forEach((message) => replayed.add(message.id));
      } else if (live.olderBefore > start) {
        this.bridge = { ...kept, until: start };
      }
    }

    this.agent = agent;
    this.replayed = replayed;
    this.replayStart = live.olderBefore;
    this.optimistic = false;
    this.pending = [];
    this.mark = live.mark;
    this.taken = live.mark.seq;
    if (this.bridge) queueMicrotask(this.loadEarlier);
  }

  private showEarlier(messages: ChatMessage[]) {
    this.replayed = new Set([...this.replayed, ...messages.map((message) => message.id)]);
    this.agent = withEarlier(this.agent, messages);
    this.earlierShown = [...messages, ...this.earlierShown];
  }

  /** Once the fetched pages reach the ones a fallen-back replay set aside, those go back above them. */
  private crossBridge(reached: bigint | undefined) {
    const bridge = this.bridge;
    if (!bridge) return;
    if (reached === bridge.until) {
      this.showEarlier(bridge.messages);
      this.cursor = bridge.cursor;
      this.bridge = undefined;
    } else if (reached === undefined || reached < bridge.until) {
      this.bridge = undefined;
    }
  }

  private deliver(text: string, attachments: Attachment[], steer = false) {
    const paths = attachments.flatMap((attachment) => (attachment.path ? [attachment.path] : []));
    if (steer) {
      this.agent = withMessage(this.agent, text, paths);
    } else {
      this.apply([{ type: 'local_prompt', text, paths }]);
      this.optimistic = true;
    }
    const entry: Outgoing = {
      key: (this.sent += 1),
      messageId: this.agent.messages[this.agent.messages.length - 1].id,
      text,
      paths,
      steer,
      state: 'sending',
    };
    this.outbox = [...this.outbox, entry];
    this.emit();
    this.push(entry);
  }

  private push(entry: Outgoing) {
    const connection = this.connection;
    Promise.resolve()
      .then(() => {
        if (!connection?.isOpen()) throw new Error('the host is not connected');
        if (!entry.steer) return connection.prompt(this.agentId, entry.text, entry.paths);
        // The turn may have ended before the message reached it, and then it starts the next one.
        return connection
          .steer(this.agentId, entry.text, entry.paths)
          .then((took) => (took ? undefined : connection.prompt(this.agentId, entry.text, entry.paths)));
      })
      .then(() => {
        this.outbox = this.outbox.filter((outgoing) => outgoing.key !== entry.key);
        this.emit();
        this.drain();
      })
      .catch((error: unknown) => {
        if (this.optimistic) {
          this.optimistic = false;
          this.agent = { ...this.agent, running: false };
        }
        this.update(entry, { state: 'failed', problem: reason(error) });
      });
  }

  private update(entry: Outgoing, change: Unsent) {
    this.outbox = this.outbox.map((outgoing) => (outgoing.key === entry.key ? { ...outgoing, ...change } : outgoing));
    this.emit();
  }

  /** Sends the messages held behind a turn once nothing is running and the chat is open. */
  private drain() {
    if (!this.queued.length || this.busy() || this.attached !== 'live' || !this.connection) return;
    const batch = nextBatch(this.queued);
    this.queued = this.queued.slice(batch.length);
    const { text, attachments } = combined(batch);
    this.deliver(text, attachments);
  }

  private ask<T>(request: (connection: ConnectionLike) => Promise<T>, what: string, then?: (answer: T) => void): Promise<void> {
    const connection = this.connection;
    return Promise.resolve()
      .then(() => {
        if (!connection?.isOpen()) throw new Error('the host is not connected');
        return request(connection);
      })
      .then((answer) => {
        then?.(answer);
        this.emit();
      })
      .catch((error: unknown) => {
        this.notice = `${what}: ${reason(error)}`;
        this.emit();
      });
  }
}
