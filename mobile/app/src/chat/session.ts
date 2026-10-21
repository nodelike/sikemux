import { ChatAttachment, MobileError, type ChatMark, type ConnectionLike } from '@sikemux/native';
import { File } from 'expo-file-system';

import { chatActions } from '@mac/chat/acpEvents';
import { recordOf } from '@mac/lib/guards';
import { earlierMessages, type EarlierPage, PAGE_TURNS, withEarlier } from '@mac/chat/earlier';
import type { QueuedMessage } from '@mac/chat/queuedMessages';
import { answerText } from '@mac/chat/answerText';
import { chatReducer, initialChatState } from '@mac/chat/reducer';
import type { AcpPermissionRequest, ChatAction, ChatState } from '@mac/chat/types';

import { problem as problemOf } from '@/core/problem';
import type { ChatInfo, CoreChatEvent } from '@/core/protocol';
import type { ChatDelivery } from '@/devices/hub';
import { parsed } from './chatEvents';
import { Folds } from './folds';
import type { ChatStore, Kept } from './kept';
import { fitting, pathsOf, uploadEach, type Attachment } from './uploads';

export type Attached = 'attaching' | 'live' | 'missing';

/** A message this phone wrote that the host has not taken yet. */
export type Unsent = { state: 'sending' | 'failed'; problem?: string };

/** A message waiting behind the running turn. */
/** A message waiting in the host's queue behind the running turn. */
export type Held = QueuedMessage;

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
  /** Where the chat is kept between launches. */
  store?: ChatStore;
  /** Told how long each step of taking the chat up took, to measure it. */
  timed?: (took: OpenTimes) => void;
};

/** In milliseconds, from when the phone began taking the chat up. */
export type OpenTimes = {
  /** Reading the chat as this phone last kept it, or null when it had not. */
  restore: number | null;
  woke: boolean;
  answered: number;
  answer: 'live' | 'resumed' | 'missing';
  bytes: number;
  applied: number;
  drawn: number;
};

type Outgoing = { key: number; messageId: string; text: string; paths: string[] } & Unsent;

/** Where the host's history of this run of the chat goes on before what the phone holds. */
type Cursor = { feed: string; before: bigint };

/** Earlier pages a fallen-back replay left a gap above, kept until the turns between are fetched. */
type Bridge = { until: bigint; pages: EarlierPage[]; cursor: Cursor | null };

const TOO_LONG = 'This chat is too long for the phone. Open it on the host.';

/**
 * A permission request that just arrived is shown before the host's list of waiting requests,
 * which comes on its own schedule, has caught up with it.
 */
const PERMISSION_GRACE_MS = 2000;

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
  /** Oldest first. */
  private earlierShown: EarlierPage[] = [];
  /** What the replay and everything since added to the transcript, played on to each earlier page. */
  private since: ChatAction[] = [];
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
  private keptAgent: ChatState | undefined;
  private restoreTook: number | null = null;

  constructor(
    readonly agentId: string,
    private readonly options: Options,
  ) {
    this.turnSince = this.now();
    const reading = performance.now();
    const kept = options.store?.load();
    if (kept) {
      this.restore(kept);
      this.restoreTook = performance.now() - reading;
    }
    this.snap = this.build();
  }

  /** Writes the chat down as drawn, unless it has not changed since it last was. */
  keep = () => {
    const store = this.options.store;
    const mark = this.mark;
    if (!store || !mark || this.agent === this.keptAgent) return;
    this.keptAgent = this.agent;
    store.save({
      agent: this.agent,
      replayed: [...this.replayed],
      mark: { feed: mark.feed, seq: String(mark.seq) },
      cursor: this.cursor && { feed: this.cursor.feed, before: String(this.cursor.before) },
      replayStart: this.replayStart === undefined ? null : String(this.replayStart),
      pages: this.pages,
    });
  };

  /** Draws the chat as this phone last kept it; the first attach asks only for what came after. */
  private restore(kept: Kept) {
    this.agent = kept.agent;
    this.keptAgent = kept.agent;
    this.replayed = new Set(kept.replayed);
    this.mark = { feed: kept.mark.feed, seq: BigInt(kept.mark.seq) };
    this.taken = this.mark.seq;
    this.cursor = kept.cursor && { feed: kept.cursor.feed, before: BigInt(kept.cursor.before) };
    this.replayStart = kept.replayStart === null ? undefined : BigInt(kept.replayStart);
    this.pages = kept.pages;
    this.attached = 'live';
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
    const { fits, notice } = fitting(this.attachments.length, picked);
    this.attachments = [...this.attachments, ...fits];
    this.notice = notice;
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
    const attachments = this.attachments.length ? await this.upload() : [];
    if (!attachments) return false;
    if (this.busy()) {
      const paths = pathsOf(attachments);
      void this.ask((connection) => connection.prompt(this.agentId, text, paths), 'Could not send the message');
      return true;
    }
    this.deliver(text, attachments);
    return true;
  };

  /**
   * Answers a question with `text` and the files picked for it, named in the answer once they are all on the
   * host. The files stay in the composer when the answer did not go out.
   */
  answerWith = async (text: string, answer: (text: string) => Promise<boolean>): Promise<boolean> => {
    this.notice = null;
    const attachments = this.attachments.length ? await this.upload() : [];
    if (!attachments) return false;
    if (await answer(answerText(text, pathsOf(attachments)))) return true;
    this.attachments = [...attachments, ...this.attachments];
    this.emit();
    return false;
  };

  retrySend = (messageId: string) => {
    const entry = this.outbox.find((outgoing) => outgoing.messageId === messageId);
    if (!entry || entry.state !== 'failed') return;
    this.update(entry, { state: 'sending', problem: undefined });
    this.push(entry);
  };

  /** Puts waiting messages into the running turn now, as one, instead of after it. */
  steer = (ids: readonly string[]) => {
    void this.ask((connection) => connection.call('steerQueued', JSON.stringify({ agentId: this.agentId, ids })), 'Could not steer');
  };

  dropQueued = (id: string) => {
    void this.ask(
      (connection) => connection.call('dropQueued', JSON.stringify({ agentId: this.agentId, ids: [id] })),
      'Could not take it back',
    );
  };

  /** Stops the running turn. Messages waiting behind it go back to the composer rather than out. */
  cancel = () => {
    const queued = this.agent.queued;
    if (queued.length) {
      const text = queued.map((held) => held.text).filter(Boolean);
      const paths = [...new Set(queued.flatMap((held) => held.paths))];
      this.draft = [...text, this.draft].filter(Boolean).join('\n\n');
      const sent = paths.flatMap((path) => {
        const attachment = this.sentFiles.get(path);
        return attachment ? [{ ...attachment, path }] : [];
      });
      this.attachments = [...sent, ...this.attachments];
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

  /** Ends the chat's agent on the host and closes it in every app. Answers whether it did. */
  stopChat = async (): Promise<boolean> => {
    let stopped = false;
    await this.ask(
      (connection) => connection.call('closeChat', JSON.stringify({ agentId: this.agentId })),
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
        this.showEarlier([{ number: this.pages, actions: parsed(page.eventsJson) }]);
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
      queued: this.agent.queued,
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

  /** Puts the picked files on the host and takes them out of the composer, or answers null when one did not get there. */
  private async upload(): Promise<Attachment[] | null> {
    if (this.uploading()) return null;
    const connection = this.connection;
    const sent = await uploadEach(
      this.attachments,
      async (attachment) => {
        if (!connection?.isOpen()) throw new Error('the host is not connected');
        const bytes = await (this.options.read ?? readFile)(attachment.uri);
        const path = await connection.attachFile(this.agentId, attachment.name, attachment.mime, bytes);
        this.sentFiles = new Map([...this.sentFiles, [path, attachment]]);
        return path;
      },
      (id, change) => this.patch(id, change),
      problemOf,
    );
    if (sent) this.attachments = [];
    return sent;
  }

  private busy() {
    return this.agent.running || this.outbox.some((outgoing) => outgoing.state === 'sending');
  }

  private apply(batch: ChatAction[]) {
    this.since.push(...batch);
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
      const taken = chatActions(JSON.parse(delivery.eventJson) as CoreChatEvent);
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
    this.applyPending();
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
    this.keep();
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

    const began = performance.now();
    let woke = false;
    const wakeAndAttach = () => {
      woke = true;
      return connection.wakeChat(agentId).then(() => connection.attachChat(agentId, this.mark));
    };
    // A chat the host says is up is taken up at once; one asleep, or gone since the host said so, is started first.
    const attaching = this.hostUp
      ? connection
          .attachChat(agentId, this.mark)
          .then((attachment) => (ChatAttachment.Missing.instanceOf(attachment) ? wakeAndAttach() : attachment))
      : wakeAndAttach();
    attaching
      .then((attachment) => {
        if (run !== this.run) return;
        const answered = performance.now() - began;
        const timed = (answer: OpenTimes['answer'], bytes: number) => {
          const report = this.options.timed;
          if (!report) return;
          const applied = performance.now() - began;
          (this.options.frame ?? animationFrame)(() =>
            report({ restore: this.restoreTook, woke, answered, answer, bytes, applied, drawn: performance.now() - began }),
          );
        };
        if (ChatAttachment.Live.instanceOf(attachment)) {
          this.replace(attachment.inner);
          timed('live', attachment.inner.replayJson.length);
        } else if (ChatAttachment.Resumed.instanceOf(attachment)) {
          this.apply(parsed(attachment.inner.eventsJson));
          this.mark = attachment.inner.mark;
          this.taken = attachment.inner.mark.seq;
          timed('resumed', attachment.inner.eventsJson.length);
        } else {
          this.problem = ChatAttachment.Restart.instanceOf(attachment) ? TOO_LONG : null;
          this.attached = 'missing';
          this.emit();
          timed('missing', 0);
          return;
        }
        this.hostUp = true;
        const waiting = held ?? [];
        held = undefined;
        this.attached = 'live';
        this.take(waiting);
        this.emit();
      })
      .catch((error: unknown) => {
        if (run !== this.run) return;
        this.problem = problemOf(error);
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
    const replay = parsed(live.replayJson);
    const rebuilt = replay.reduce(chatReducer, initialChatState);
    const replayed = new Set(rebuilt.messages.map((message) => message.id));
    const wasRunning = this.agent.running;
    let agent = chatReducer(rebuilt, {
      type: 'ready',
      capabilities: recordOf(JSON.parse(live.capabilitiesJson)) ?? {},
      setup: recordOf(JSON.parse(live.setupJson)) ?? {},
    });
    // A turn the replay already opened goes on: starting it again would cut the answer streaming into it in two.
    if (live.running && !rebuilt.running) agent = chatReducer(agent, { type: 'turn_started' });
    if (live.running && !wasRunning) this.turnSince = this.now();

    // The replay has what the host took; what it did not take is put back after it.
    this.outbox = this.outbox.map((outgoing) => {
      agent = withMessage(agent, outgoing.text, outgoing.paths);
      return { ...outgoing, messageId: agent.messages[agent.messages.length - 1].id };
    });

    const sameFeed = this.mark?.feed === live.mark.feed;
    const before = this.earlierShown;
    const start = this.replayStart;
    const kept = { pages: before, cursor: this.cursor };
    this.earlierShown = [];
    this.since = replay;
    this.bridge = undefined;
    this.cursor = live.olderBefore === undefined ? null : { feed: live.mark.feed, before: live.olderBefore };
    this.agent = agent;
    this.replayed = replayed;
    if (sameFeed && before.length && start !== undefined && live.olderBefore !== undefined) {
      if (live.olderBefore === start) {
        this.showEarlier(before);
        this.cursor = kept.cursor;
      } else if (live.olderBefore > start) {
        this.bridge = { ...kept, until: start };
      }
    }

    this.replayStart = live.olderBefore;
    this.optimistic = false;
    this.pending = [];
    this.mark = live.mark;
    this.taken = live.mark.seq;
    if (this.bridge) queueMicrotask(this.loadEarlier);
  }

  /** Puts `pages`, oldest first and all from before what is shown, above the chat. */
  private showEarlier(pages: EarlierPage[]) {
    let later = [...this.earlierShown.flatMap((page) => page.actions), ...this.since];
    for (const page of [...pages].reverse()) {
      const messages = earlierMessages(page, later);
      this.replayed = new Set([...this.replayed, ...messages.map((message) => message.id)]);
      this.agent = withEarlier(this.agent, messages);
      later = [...page.actions, ...later];
    }
    this.earlierShown = [...pages, ...this.earlierShown];
  }

  /** Once the fetched pages reach the ones a fallen-back replay set aside, those go back above them. */
  private crossBridge(reached: bigint | undefined) {
    const bridge = this.bridge;
    if (!bridge) return;
    if (reached === bridge.until) {
      this.showEarlier(bridge.pages);
      this.cursor = bridge.cursor;
      this.bridge = undefined;
    } else if (reached === undefined || reached < bridge.until) {
      this.bridge = undefined;
    }
  }

  private deliver(text: string, attachments: Attachment[]) {
    const paths = pathsOf(attachments);
    this.apply([{ type: 'local_prompt', text, paths }]);
    this.optimistic = true;
    const entry: Outgoing = {
      key: (this.sent += 1),
      messageId: this.agent.messages[this.agent.messages.length - 1].id,
      text,
      paths,
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
        return connection.prompt(this.agentId, entry.text, entry.paths);
      })
      .then(() => {
        this.outbox = this.outbox.filter((outgoing) => outgoing.key !== entry.key);
        this.emit();
      })
      .catch((error: unknown) => {
        if (this.optimistic) {
          this.optimistic = false;
          this.agent = { ...this.agent, running: false };
        }
        this.update(entry, { state: 'failed', problem: problemOf(error) });
      });
  }

  private update(entry: Outgoing, change: Unsent) {
    this.outbox = this.outbox.map((outgoing) => (outgoing.key === entry.key ? { ...outgoing, ...change } : outgoing));
    this.emit();
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
        this.notice = `${what}: ${problemOf(error)}`;
        this.emit();
      });
  }
}
