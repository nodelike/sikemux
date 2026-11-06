import { ChatAttachment, type ConnectionLike } from '@sikemux/native';
import { File } from 'expo-file-system';

import { answerText } from '@/chat/answerText';
import { applyChanges, type ChatRows } from '@shared/chat/rows';
import type { ChatChange, ChatView, PermissionAsk, ToolDetail } from '@shared/chat/types';
import type { ChatInfo, FindAnswer, FindParams } from '@shared/wire.generated';

import { callHost } from '@/core/hostCalls';
import { problem as problemOf } from '@/core/problem';
import type { ChatDelivery } from '@/devices/hub';
import { Draft } from './draft';
import { Feed } from './feed';
import { Folds } from './folds';
import type { ChatStore, DraftStore } from './kept';
import type { DraftToken } from './mentions';
import { Outbox, type Unsent } from './outbox';
import { pathsOf, type Attachment } from './uploads';
import type { StashItem } from '@/stash/api';
import { withContext, type ContextChip } from '@/stash/words';

export type Attached = 'attaching' | 'live' | 'missing';

export type ChatSnapshot = {
  /** The chat as the host folded it, with what this phone sent and the host does not show yet after it. */
  view: ChatRows;
  /** The permission requests still waiting on the host. */
  permissions: PermissionAsk[];
  attached: Attached;
  /** Why the chat could not be opened, in the host's words. */
  problem: string | null;
  /** Something this phone asked for that did not happen, such as a stop or a setting. */
  notice: string | null;
  /** By message id. */
  unsent: ReadonlyMap<string, Unsent>;
  /** Picked for the next message. */
  attachments: readonly Attachment[];
  /** Links and text the draft carries, as the host keeps them: shown as chips, sent as words. */
  chips: readonly ContextChip[];
  /** The agents, files, chats and issues the draft names, over the words that name them. */
  tokens: readonly DraftToken[];
  /** What this phone sent, by the path the host keeps it at. */
  sentFiles: ReadonlyMap<string, Attachment>;
  /** Permission requests whose answer is on its way. */
  answering: ReadonlySet<string>;
  /** Background tasks this phone asked the host to stop. */
  stopping: ReadonlySet<string>;
  /** This phone asked the running turn to stop and the host has not ended it yet. */
  stoppingTurn: boolean;
  /** How the chat asks before acting, as this phone last set it or the host last said; null before either. */
  permissionMode: string | null;
  hasEarlier: boolean;
  earlier: 'idle' | 'loading' | 'failed';
  /** When the running turn began. */
  turnSince: number;
};

type Options = {
  /** Hands over this host's chat changes as they arrive. */
  listen: (take: (deliveries: ChatDelivery[]) => void) => () => void;
  /** Runs once before the next frame is drawn; returns how to call that off. */
  frame?: (run: () => void) => () => void;
  now?: () => number;
  /** The contents of a file on this phone. */
  read?: (uri: string) => Promise<ArrayBuffer>;
  /** Where the chat is kept between launches. */
  store?: ChatStore;
  /** Where writing not saved on the host yet is kept, so it outlives the app quitting. */
  drafts?: DraftStore;
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

function readFile(uri: string): Promise<ArrayBuffer> {
  return new File(uri).arrayBuffer();
}

/**
 * One chat on one host, kept outside React so it outlives the screen: the chat as the host folds it,
 * where its changes got to, what this phone sent and what it still means to send.
 */
export class ChatSession {
  readonly folds = new Folds();
  private readonly feed: Feed;
  private readonly draft: Draft;
  private readonly outbox: Outbox;
  private listeners = new Set<() => void>();
  private attached: Attached = 'attaching';
  private problem: string | null = null;
  private notice: string | null = null;
  private stoppingTurn = false;
  /** Model and effort picks this phone made, by picker id, shown until the host says what it runs on. */
  private chosen: ReadonlyMap<string, string> = new Map();
  private answering: ReadonlySet<string> = new Set();
  private stopping: ReadonlySet<string> = new Set();
  /** The mode this phone set, until the host says another. */
  private mode: string | null = null;
  /** When this phone saw the running turn begin, for a turn the host has no time for. */
  private turnSince: number;

  /** What the host lists as waiting, or null before it has said. */
  private hostUp: boolean | undefined;

  private connection: ConnectionLike | undefined;
  private holders = 0;
  /** Counts attaches, so an answer that comes back after the chat was taken up again is dropped. */
  private run = 0;
  private stopRun: (() => void) | undefined;
  private snap: ChatSnapshot;
  private viewOf: { over: ChatRows; chosen: ReadonlyMap<string, string>; view: ChatRows } | undefined;
  private restoreTook: number | null = null;

  constructor(
    readonly agentId: string,
    private readonly options: Options,
  ) {
    this.turnSince = this.now();
    this.feed = new Feed({
      frame: options.frame,
      store: options.store,
      applied: (before) => this.applied(before),
      emit: () => this.emit(),
      fellBehind: () => this.restart(),
    });
    this.draft = new Draft(agentId, {
      store: options.drafts,
      held: () => this.feed.chat.draft,
      live: () => (this.connection?.isOpen() && this.attached === 'live' ? this.connection : undefined),
      emit: () => this.emit(),
    });
    this.outbox = new Outbox(agentId, {
      connection: () => this.connection,
      chat: () => this.feed.chat,
      read: options.read ?? readFile,
      now: () => this.now(),
      emit: () => this.emit(),
    });
    const reading = performance.now();
    const kept = options.store?.load();
    if (kept) {
      this.feed.restore(kept);
      this.attached = 'live';
      this.draft.takeIn();
      this.restoreTook = performance.now() - reading;
    }
    this.snap = this.build();
  }

  /** Writes the chat down as drawn, unless it has not changed since it last was. */
  keep = () => {
    this.feed.keep();
  };

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  snapshot = () => this.snap;

  draftText = () => this.draft.text;

  setDraft = (text: string) => {
    if (text === this.draft.text) return;
    if (this.draft.write(text)) this.emit();
    else this.notify();
  };

  /** Writes what was picked from the mention menu over the word from `start` that asked for it, as a chip when it is one. */
  mention = (start: number, pick: { written: string; id?: string; label?: string; context?: ContextChip }) => {
    this.draft.mention(start, pick);
    this.emit();
  };

  /** Keeps what the issue or pull request chip `id` sends, once the host has read it. */
  track = (id: string, context: ContextChip) => {
    this.draft.track(id, context);
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

  /** What the host's own view says of this chat. A chat that comes back after it slept, stopped or restarted is taken up again. */
  hostSaw = (info: ChatInfo | undefined) => {
    const up = info?.state === 'ready';
    const cameBack = this.hostUp === false && up;
    this.hostUp = up;
    if (cameBack && this.connection && this.attached !== 'attaching') this.restart();
  };

  /** Adds what was picked for the next message, leaving out what the host would refuse. */
  attach = (picked: Attachment[]) => {
    this.notice = this.draft.attach(picked);
    this.emit();
  };

  /** Has the host put stash items in this chat's draft: files beside it, links and text as chips, notes in its words. */
  attachStash = async (items: readonly StashItem[]) => {
    const connection = this.openConnection();
    for (const item of items) await callHost(connection, 'attachStash', { id: item.id, agentId: this.agentId });
  };

  removeChip = (uri: string) => {
    this.draft.removeChip(uri);
    this.emit();
  };

  /**
   * Stashes the whole draft as one prompt, with the files picked here put on the host first, and empties
   * it. Answers the prompt's id.
   */
  stashPrompt = async (): Promise<string> => {
    const connection = this.openConnection();
    const draft = this.draft;
    if (draft.attachments.some((attachment) => !attachment.path)) {
      draft.attachments = await this.outbox.put(connection, draft.attachments, (id, change) => draft.patch(id, change));
      draft.edited();
    }
    if (!(await draft.saveNow())) throw new Error('the draft did not reach the host');
    const { id } = await callHost(connection, 'stashPrompt', { agentId: this.agentId });
    draft.clear();
    this.emit();
    return id;
  };

  /** Puts a stashed prompt back in the draft and takes it out of the stash. */
  unstashPrompt = async (id: string) => {
    const connection = this.openConnection();
    await callHost(connection, 'attachStash', { id, agentId: this.agentId });
    await callHost(connection, 'removeStash', { id });
  };

  removeAttachment = (id: string) => {
    if (this.draft.removeAttachment(id)) this.emit();
  };

  /**
   * Shows the message at once, behind the running turn when there is one or into it when `steers`, and puts
   * its files on the host before sending it. Answers whether the composer can let it go.
   */
  send = async (text: string, steers = false): Promise<boolean> => {
    if (this.draft.uploading()) return false;
    this.notice = null;
    const message = this.draft.takeOut(text);
    if (this.outbox.waiting()) this.outbox.deliver(message.text, message.files, true, message.named);
    else if (this.busy()) this.queue(message.text, message.files, steers && this.feed.chat.running, message.named);
    else this.outbox.deliver(message.text, message.files, false, message.named);
    return true;
  };

  /** Shows a message begun on another screen at once, with its files already on the host, and sends it once the chat is attached. */
  sendWhenAttached = (text: string, uploaded: Attachment[]) => {
    this.outbox.deliver(text, uploaded, true);
  };

  /**
   * Answers a question with `text` and the files picked for it, named in the answer once they are all on the
   * host. The files stay in the composer when the answer did not go out.
   */
  answerWith = async (text: string, answer: (text: string) => Promise<boolean>): Promise<boolean> => {
    this.notice = null;
    const draft = this.draft;
    const attachments = draft.attachments.length ? await this.upload() : [];
    if (!attachments) return false;
    if (await answer(answerText(withContext(text, draft.chips), pathsOf(attachments)))) {
      draft.chips = [];
      return true;
    }
    draft.putBack([], attachments);
    this.emit();
    return false;
  };

  retrySend = (messageId: string) => {
    this.outbox.retry(messageId);
  };

  /** Puts waiting messages into the running turn now, as one, instead of after it. */
  steer = (ids: readonly string[]) => {
    ids = this.outbox.onHostQueue(ids);
    if (!ids.length) return;
    void this.ask((connection) => callHost(connection, 'steerQueued', { agentId: this.agentId, ids: [...ids] }), 'Could not steer');
  };

  /** Acts on the turn that failed: sends it again, lets it go, or keeps it from resuming once a usage limit resets. */
  failedTurn = (method: 'retryFailed' | 'discardFailed' | 'cancelResume') => {
    void this.ask((connection) => callHost(connection, method, { agentId: this.agentId }), 'Could not do that');
  };

  /** Carries the chat on in another of the person's accounts, a turn that failed sent again there. */
  moveToAccount = (accountId: string) => {
    void this.ask((connection) => callHost(connection, 'moveChatAccount', { agentId: this.agentId, accountId }), 'Could not move the chat');
  };

  dropQueued = (id: string) => {
    if (!this.outbox.onHostQueue([id]).length) return;
    void this.ask((connection) => callHost(connection, 'dropQueued', { agentId: this.agentId, ids: [id] }), 'Could not take it back');
  };

  /** Stops the running turn. Messages waiting behind it go back to the composer rather than out. */
  cancel = () => {
    const chat = this.feed.chat;
    const queued = chat.queued;
    if (queued.length) {
      const text = queued.map((held) => held.text).filter(Boolean);
      const paths = [...new Set(queued.flatMap((held) => held.paths))];
      const sent = paths.flatMap((path) => {
        const attachment = this.outbox.sentFiles.get(path);
        return attachment ? [{ ...attachment, path }] : [];
      });
      this.draft.putBack(text, sent);
      this.draft.edited();
    }
    this.notice = null;
    if (chat.running) this.stoppingTurn = true;
    this.emit();
    this.ask(
      (connection) => connection.cancel(this.agentId),
      'Could not stop the turn',
      undefined,
      () => {
        this.stoppingTurn = false;
      },
    );
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
        const chat = this.feed.chat;
        this.feed.chat = { ...chat, permissions: chat.permissions.filter((request) => request.requestId !== requestId) };
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
    if (this.feed.chat.changingMode) return;
    const before = this.mode;
    this.mode = mode;
    this.notice = null;
    this.emit();
    this.ask(
      (connection) => connection.setPermissionMode(this.agentId, mode),
      'Could not change permissions',
      undefined,
      () => {
        if (this.mode === mode) this.mode = before;
      },
    );
  };

  /** Ends the chat's agent on the host and closes it in every app. Answers whether it did. */
  stopChat = async (): Promise<boolean> => {
    let stopped = false;
    await this.ask(
      (connection) => callHost(connection, 'closeChat', { agentId: this.agentId }),
      'Could not stop the chat',
      () => {
        stopped = true;
      },
    );
    return stopped;
  };

  /** Shows the pick at once, and goes back to what the host runs on if it refuses it. */
  setConfig = (configId: string, value: string) => {
    this.notice = null;
    this.chosen = new Map(this.chosen).set(configId, value);
    this.emit();
    this.ask(
      (connection) => connection.setChatConfig(this.agentId, configId, value),
      'Could not change the setting',
      undefined,
      () => {
        if (this.chosen.get(configId) !== value) return;
        const chosen = new Map(this.chosen);
        chosen.delete(configId);
        this.chosen = chosen;
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

  /**
   * Takes the chat back to before the person's message `rowId` and sends `text` in its place with the files it
   * carried. The message and everything after it leave at once; if the host refuses, the chat is read again whole.
   */
  edit = (rowId: string, text: string) => {
    const row = this.feed.chat.messages.find((message) => message.id === rowId);
    const messageId = row?.promptId;
    if (!row || !messageId || !text) return;
    this.notice = null;
    this.feed.apply((chat) => applyChanges(chat, [{ op: 'drop', from: rowId }]));
    this.emit();
    void this.ask(
      (connection) => callHost(connection, 'editMessage', { agentId: this.agentId, messageId, text, paths: row.attachments ?? [] }),
      'Could not edit the message',
      undefined,
      this.retry,
    );
  };

  /** Fetches the rows before the first one shown, unless that is already under way. */
  loadEarlier = () => {
    void this.feed.pageBack(this.connection, this.agentId);
  };

  /** Pages back until row `id` is held, answering whether it is. */
  reach = async (id: string): Promise<boolean> => {
    while (!this.feed.chat.messages.some((message) => message.id === id)) {
      if (!(await this.feed.pageBack(this.connection, this.agentId))) return false;
    }
    return true;
  };

  /** Asks the host to find words across the whole chat, held here or not, from where `params` says. */
  find = async (params: Omit<FindParams, 'agentId' | 'feed'>): Promise<FindAnswer> => {
    const feed = this.feed.mark?.feed;
    if (feed === undefined) throw new Error('the chat is not open');
    return callHost(this.openConnection(), 'chatFind', { ...params, agentId: this.agentId, feed });
  };

  /** A call's whole change and what it printed, which rows carry only counts of. `done` calls are read once. */
  toolDetail = (toolCallId: string, done: boolean): Promise<ToolDetail> =>
    this.feed.toolDetail(this.connection, this.agentId, toolCallId, done);

  /** Opens the chat again from the start, after it could not be opened. */
  retry = () => {
    this.feed.forget();
    if (this.connection) this.restart();
  };

  /** Writes down what is not saved on the host yet, so it outlives the app quitting. */
  keepDraft = () => {
    this.draft.keep();
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
    const chat = this.feed.chat;
    const answering = this.answering;
    const all = chat.permissions;
    const permissions = all.filter((request) => !answering.has(request.requestId));
    const draft = this.draft;
    return {
      view: this.view(),
      permissions: permissions.length === all.length ? all : permissions,
      attached: this.attached,
      problem: this.problem,
      notice: this.notice,
      unsent: this.outbox.unsent(),
      attachments: draft.attachments,
      chips: draft.chips,
      tokens: draft.tokens,
      sentFiles: this.outbox.sentFiles,
      answering: this.answering,
      stopping: this.stopping,
      stoppingTurn: this.stoppingTurn,
      permissionMode: this.mode ?? (chat.permissionMode || null),
      hasEarlier: chat.olderBefore !== null,
      earlier: this.feed.earlier,
      turnSince: this.since(),
    };
  }

  /** The chat as the host folds it, with what this phone sent and picked that the host does not show yet. */
  private view(): ChatRows {
    const over = this.outbox.over(this.feed.chat);
    const chosen = this.chosen;
    const kept = this.viewOf;
    if (kept?.over === over && kept.chosen === chosen) return kept.view;
    const picked = (picker: ChatRows['model']) => {
      const value = picker && chosen.get(picker.id);
      return picker && value !== undefined ? { ...picker, current: value } : picker;
    };
    const view = chosen.size ? { ...over, model: picked(over.model), effort: picked(over.effort) } : over;
    this.viewOf = { over, chosen, view };
    return view;
  }

  /** When the running turn began: as the host timed it, or else as this phone saw it. */
  private since(): number {
    const { turnOpening, messages } = this.view();
    if (turnOpening?.at) return turnOpening.at;
    for (let at = messages.length - 1; at >= 0; at -= 1) {
      const row = messages[at];
      if (row.role === 'user') return row.sentAt ?? this.turnSince;
      if (row.opensTurn) return row.opensTurn.at ?? this.turnSince;
    }
    return this.turnSince;
  }

  private openConnection(): ConnectionLike {
    const connection = this.connection;
    if (!connection?.isOpen() || this.attached !== 'live') throw new Error('the host is not connected');
    return connection;
  }

  /** Puts the picked files on the host and takes them out of the composer, or answers null when one did not get there. */
  private async upload(): Promise<Attachment[] | null> {
    const draft = this.draft;
    if (draft.uploading()) return null;
    try {
      const sent = await this.outbox.put(this.connection, draft.attachments, (id, change) => draft.patch(id, change));
      draft.attachments = [];
      return sent;
    } catch {
      return null;
    }
  }

  private busy() {
    return this.feed.chat.running || this.outbox.sending();
  }

  /** Sends behind the running turn; a message that does not go goes back to the composer. */
  private queue(...message: Parameters<Outbox['queue']>) {
    const [text, files] = message;
    this.outbox.queue(...message).catch((error: unknown) => {
      this.draft.putBack([text], files);
      this.notice = `Could not send the message: ${problemOf(error)}`;
      this.emit();
    });
  }

  /** What follows from the host's changes, and letting go of what this phone sent once the host shows it. */
  private applied(before: ChatRows) {
    const chat = this.feed.chat;
    if (!before.running && chat.running) this.turnSince = this.now();
    if (!chat.running) this.stoppingTurn = false;
    if (chat.permissionMode !== before.permissionMode) this.mode = null;
    if (chat.queued !== before.queued || chat.running !== before.running) this.outbox.dropTaken();
    if (this.chosen.size && (chat.model !== before.model || chat.effort !== before.effort)) this.chosen = new Map();
    this.outbox.settle(before.running && !chat.running);
    if (chat.draft !== before.draft) this.draft.takeIn();
  }

  private restart() {
    this.stop();
    this.start();
  }

  private stop() {
    this.run += 1;
    this.stopRun?.();
    this.stopRun = undefined;
    this.feed.cut();
    this.feed.keep();
  }

  private start() {
    const connection = this.connection;
    if (!connection) return;
    const run = (this.run += 1);
    const agentId = this.agentId;
    const feed = this.feed;
    if (!feed.mark || this.attached === 'missing') this.attached = 'attaching';
    this.problem = null;
    this.emit();
    let held: ChatDelivery[] | undefined = [];
    const off = this.options.listen((deliveries) => {
      if (run !== this.run) return;
      const mine = deliveries.filter((delivery) => delivery.agentId === agentId);
      if (!mine.length) return;
      // Until the attach answer says where the chat it sends stands, changes wait.
      if (held) held.push(...mine);
      else feed.take(mine);
    });
    this.stopRun = () => {
      off();
      // What arrived but was not applied yet goes into the chat now, so the next attach picks up after it.
      feed.applyPending();
      if (connection.isOpen()) connection.detachChat(agentId).catch(() => {});
    };

    const began = performance.now();
    let woke = false;
    const wakeAndAttach = () => {
      woke = true;
      return connection.wakeChat(agentId).then(() => connection.attachChat(agentId, feed.mark));
    };
    // A chat the host says is up is taken up at once; one asleep, or gone since the host said so, is started first.
    const attaching = this.hostUp
      ? connection
          .attachChat(agentId, feed.mark)
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
          feed.frame(() => report({ restore: this.restoreTook, woke, answered, answer, bytes, applied, drawn: performance.now() - began }));
        };
        if (ChatAttachment.Live.instanceOf(attachment)) {
          feed.openAt(JSON.parse(attachment.inner.viewJson) as ChatView, attachment.inner.mark);
          timed('live', attachment.inner.viewJson.length);
        } else if (ChatAttachment.Resumed.instanceOf(attachment)) {
          feed.resumeAt(JSON.parse(attachment.inner.changesJson) as ChatChange[], attachment.inner.mark);
          timed('resumed', attachment.inner.changesJson.length);
        } else {
          this.attached = 'missing';
          // The chat kept on this phone may hold requests nothing on the host waits on any more.
          feed.apply((chat) => ({ ...chat, permissions: [] }));
          this.outbox.failWaiting('the chat is gone from the host');
          this.emit();
          timed('missing', 0);
          return;
        }
        this.hostUp = true;
        const waiting = held ?? [];
        held = undefined;
        this.attached = 'live';
        feed.take(waiting);
        this.outbox.pushWaiting();
        void this.draft.save();
        this.emit();
      })
      .catch((error: unknown) => {
        if (run !== this.run) return;
        this.problem = problemOf(error);
        this.attached = 'missing';
        this.outbox.failWaiting(this.problem);
        this.emit();
      });
  }

  private ask<T>(
    request: (connection: ConnectionLike) => Promise<T>,
    what: string,
    then?: (answer: T) => void,
    undo?: () => void,
  ): Promise<void> {
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
        undo?.();
        this.notice = `${what}: ${problemOf(error)}`;
        this.emit();
      });
  }
}
