import type { ConnectionLike } from '@sikemux/native';
import { randomUUID } from 'expo-crypto';

import type { ChatRows } from '@shared/chat/rows';
import type { Say } from '@shared/lib/say';
import type { ChatMessage, QueuedMessage } from '@shared/wire.generated';

import { callHost } from '@/core/hostCalls';
import { problem as problemOf } from '@/core/problem';
import type { ContextChip } from '@/stash/words';
import type { Named } from './draft';
import type { OutboxStore, SavedOutboxEntry } from './kept';
import { pathsOf, uploadEach, type Attachment } from './uploads';

/** A message this phone wrote that the host has not taken yet. */
export type Unsent = { state: 'sending' | 'failed'; problem?: string };

/** A message waiting in the host's queue behind the running turn. */
export type Held = QueuedMessage;

/**
 * `sent` is taken by the host and not shown by it yet. Its `files` go up behind it, each named in the row by the
 * host's path once it is there and by this phone's copy before. `waiting` was sent before the chat was attached,
 * and goes out once it is.
 */
type Outgoing = {
  messageId: string;
  text: string;
  files: Attachment[];
  /** The host's files its chips name, and the agents, chats and issues they send. */
  paths: string[];
  context: ContextChip[];
  at: number;
  state: Unsent['state'] | 'sent';
  problem?: string;
  waiting?: boolean;
  /** Kept on disk until the host acknowledges or settles it, so kills mid-upload or on failure never lose it. */
  kept?: boolean;
  steers?: boolean;
};

/** A message sent behind the running turn, shown in the queue until the host's own queue holds it. */
type Holding = Held & { taken: boolean };

const shownPath = (file: Attachment) => file.path ?? file.uri;

const NAMED_NONE: Named = { paths: [], context: [] };

/** The messages in `holding` the host's queue does not hold yet. */
function notQueued(holding: readonly Holding[], host: readonly Held[]): Holding[] {
  return holding.filter((held) => !host.some((queued) => queued.id === held.id));
}

function ownRow(outgoing: Outgoing): ChatMessage {
  return {
    id: outgoing.messageId,
    role: 'user',
    promptId: outgoing.messageId,
    parts: outgoing.text.trim() ? [{ kind: 'text', id: `${outgoing.messageId}/text`, text: outgoing.text }] : [],
    attachments: [...outgoing.files.map(shownPath), ...outgoing.paths],
    ...(outgoing.context.length ? { context: outgoing.context.map(({ uri, title }) => ({ uri, title })) } : {}),
    sentAt: outgoing.at,
  };
}

type Host = {
  say: Say;
  store?: OutboxStore;
  onWaiting?: (hasWaiting: boolean) => void;
  connection: () => ConnectionLike | undefined;
  /** Whether the chat is currently attached on its open connection. */
  attached?: () => boolean;
  /** The chat as the host last folded it. */
  chat: () => ChatRows;
  /** The contents of a file on this phone. */
  read: (uri: string) => Promise<ArrayBuffer>;
  now: () => number;
  emit: () => void;
};

/** What this phone sent that the host does not show yet, sent in order however long each one's files take. */
export class Outbox {
  /** What this phone sent, by the path the host keeps it at. */
  sentFiles: ReadonlyMap<string, Attachment> = new Map();
  private entries: Outgoing[] = [];
  private holding: Holding[] = [];
  /** Bumped when the host is forgotten or unpaired so in-flight rejections never re-persist or re-watch. */
  private epoch = 0;
  /** Messages reach the host in the order they were sent, however long their files take. */
  private line: Promise<void> = Promise.resolve();
  private overOf: { chat: ChatRows; entries: Outgoing[]; holding: Holding[]; view: ChatRows } | undefined;
  private unsentOf: { entries: Outgoing[]; unsent: ReadonlyMap<string, Unsent> } | undefined;

  constructor(
    private readonly agentId: string,
    private readonly host: Host,
  ) {
    const saved = host.store?.load() ?? [];
    if (saved.length) {
      this.entries = saved.map((entry) => ({
        messageId: entry.messageId,
        text: entry.text,
        files: entry.files,
        paths: entry.paths,
        context: entry.context,
        at: entry.at,
        state: entry.state === 'failed' ? 'failed' : 'sending',
        problem: entry.state === 'failed' ? entry.problem : undefined,
        waiting: entry.state !== 'failed',
        kept: true,
        steers: entry.steers,
      }));
      saved.forEach((entry) => this.show(entry.files));
    }
  }

  private notifyPending() {
    this.host.onWaiting?.(this.waiting() || this.sending());
  }

  private persistWaiting() {
    const saved: SavedOutboxEntry[] = this.entries
      .filter((entry) => (entry.waiting || entry.kept) && entry.state !== 'sent')
      .map(({ messageId, text, files, paths, context, at, steers, state, problem }) => ({
        messageId,
        text,
        files,
        paths,
        context,
        at,
        ...(steers ? { steers } : {}),
        ...(state === 'failed' ? { state: 'failed' as const, ...(problem ? { problem } : {}) } : { state: 'waiting' as const }),
      }));
    if (saved.length) this.host.store?.save(saved);
    else this.host.store?.clear();
    this.notifyPending();
  }

  /** Drops all in-memory and on-disk pending messages (used when a host is forgotten or unpaired). */
  clearAll() {
    this.epoch += 1;
    this.entries = [];
    this.holding = [];
    this.host.store?.clear();
    this.notifyPending();
    this.host.emit();
  }

  /** Some message is still on its way. */
  sending() {
    return this.entries.some((outgoing) => outgoing.state !== 'failed');
  }

  /** Some message waits for the chat to be attached. */
  waiting() {
    return this.entries.some((outgoing) => outgoing.waiting);
  }

  /** `chat` with what this phone sent that the host does not show yet. */
  over(chat: ChatRows): ChatRows {
    const { entries, holding } = this;
    const kept = this.overOf;
    if (kept?.chat === chat && kept.entries === entries && kept.holding === holding) return kept.view;
    const behind = holding.length ? notQueued(holding, chat.queued) : [];
    const view =
      entries.length || behind.length
        ? {
            ...chat,
            messages: entries.length ? [...chat.messages, ...entries.map(ownRow)] : chat.messages,
            running: chat.running || this.sending(),
            queued: behind.length ? [...chat.queued, ...behind] : chat.queued,
          }
        : chat;
    this.overOf = { chat, entries, holding, view };
    return view;
  }

  /** By message id. */
  unsent(): ReadonlyMap<string, Unsent> {
    const entries = this.entries;
    if (this.unsentOf?.entries !== entries) {
      const unsent = new Map(
        entries.flatMap(({ messageId, state, problem }) => (state === 'sent' ? [] : [[messageId, { state, problem }]])),
      );
      this.unsentOf = { entries, unsent };
    }
    return this.unsentOf.unsent;
  }

  deliver(text: string, files: Attachment[], waiting = false, named: Named = NAMED_NONE, steers = false) {
    this.show(files);
    const entry: Outgoing = {
      messageId: randomUUID(),
      text,
      files,
      ...named,
      at: this.host.now(),
      state: 'sending',
      waiting,
      kept: waiting,
      ...(steers ? { steers } : {}),
    };
    this.entries = [...this.entries, entry];
    if (waiting) this.persistWaiting();
    this.host.emit();
    if (!waiting) this.push(entry);
  }

  retry(messageId: string) {
    const entry = this.entries.find((outgoing) => outgoing.messageId === messageId);
    if (!entry || entry.state !== 'failed') return;
    this.update(messageId, { state: 'sending', problem: undefined });
    this.push(entry);
  }

  /** Sends what was shown before the chat was attached, now that it is. */
  pushWaiting() {
    const waiting = this.entries.filter((outgoing) => outgoing.waiting);
    if (!waiting.length) return;
    const chat = this.host.chat();
    const shown = new Set(chat.messages.flatMap((row) => (row.promptId ? [row.promptId] : [])));
    const queued = new Set(chat.queued.map((held) => held.id));
    const alreadyLanded = (id: string) => shown.has(id) || queued.has(id);
    this.entries = this.entries
      .filter((outgoing) => !(outgoing.waiting && alreadyLanded(outgoing.messageId)))
      .map((outgoing) => (outgoing.waiting ? { ...outgoing, waiting: false } : outgoing));
    this.persistWaiting();
    const toSend = waiting.filter((entry) => !alreadyLanded(entry.messageId));
    if (!toSend.length) {
      this.host.emit();
      return;
    }
    toSend.forEach((entry) => this.push(entry));
  }

  failWaiting(problem: string) {
    if (!this.waiting()) return;
    this.entries = this.entries.map((outgoing) =>
      outgoing.waiting ? { ...outgoing, waiting: false, state: 'failed', problem } : outgoing,
    );
    this.persistWaiting();
  }

  /**
   * Sends the message behind the running turn, or into it when it `steers`, showing it queued at once. Rejects when
   * it did not go, once it is out of the queue.
   */
  queue(text: string, files: Attachment[], steers: boolean, named: Named): Promise<void> {
    const id = randomUUID();
    this.show(files);
    const paths = [...files.map(shownPath), ...named.paths];
    const context = named.context.map(({ uri, title }) => ({ uri, title }));
    this.holding = [...this.holding, { id, text, paths, context, taken: false }];
    this.host.emit();
    const connection = this.host.connection();
    const generation = this.epoch;
    return this.inOrder(async () => {
      if (this.epoch !== generation) return;
      if (!connection?.isOpen()) throw new Error(this.host.say('the host is not connected'));
      const sent = await this.put(connection, files, () => {});
      if (this.epoch !== generation) return;
      const paths = [...pathsOf(sent), ...named.paths];
      if (!steers) return this.prompt(connection, id, text, paths, named.context);
      const ids = this.onHostQueue(this.host.chat().queued.map((held) => held.id));
      const message = { text, paths, context: named.context };
      await callHost(connection, 'steerQueued', { agentId: this.agentId, ids, message });
    })
      .then(() => {
        if (this.epoch !== generation) return;
        const held = this.holding.find((holding) => holding.id === id);
        if (held && (steers || !notQueued([held], this.host.chat().queued).length))
          this.holding = this.holding.filter((holding) => holding !== held);
        else this.holding = this.holding.map((holding) => (holding.id === id ? { ...holding, taken: true } : holding));
        this.host.emit();
      })
      .catch((error: unknown) => {
        this.holding = this.holding.filter((holding) => holding.id !== id);
        if (this.epoch !== generation) return;
        // Only non-steering messages carry a messageId on the wire for reconnect deduplication;
        // never silently re-send a steer whose wire call may already have landed.
        if (!steers && !connection?.isOpen()) {
          this.entries = [
            ...this.entries,
            {
              messageId: id,
              text,
              files,
              ...named,
              at: this.host.now(),
              state: 'sending',
              waiting: true,
              kept: true,
            },
          ];
          this.persistWaiting();
          this.host.emit();
          const current = this.host.connection();
          if (current && current !== connection && current.isOpen() && (this.host.attached?.() ?? true)) {
            this.pushWaiting();
          }
          return;
        }
        throw error;
      });
  }

  /** Lets go of queued messages the host has taken, now that its own queue or turn says where they went. */
  dropTaken() {
    if (this.holding.some((held) => held.taken)) this.holding = this.holding.filter((held) => !held.taken);
  }

  /** Of `ids`, the ones in the host's queue rather than still on their way to it. */
  onHostQueue(ids: readonly string[]) {
    const onTheirWay = notQueued(this.holding, this.host.chat().queued);
    return ids.filter((id) => !onTheirWay.some((held) => held.id === id));
  }

  /* The host shows a message as the row that carries its id; one that met a running turn waits in
     the host's queue under it instead. Once a turn has ended, nothing the host took is still to come. */
  settle(turnEnded = false) {
    if (!this.entries.length) return;
    const chat = this.host.chat();
    const shown = new Set(chat.messages.flatMap((row) => (row.promptId ? [row.promptId] : [])));
    const queued = new Set(chat.queued.map((held) => held.id));
    const kept = this.entries.filter((outgoing) => {
      if (shown.has(outgoing.messageId) || queued.has(outgoing.messageId)) return false;
      if (turnEnded && outgoing.state === 'sent') return false;
      return true;
    });
    if (kept.length !== this.entries.length) {
      const hadPersisted = this.entries.some((outgoing) => outgoing.waiting || outgoing.kept);
      this.entries = kept;
      if (hadPersisted) this.persistWaiting();
      else this.notifyPending();
    }
  }

  /** Puts each of `files` the host does not have yet on it, answering them all with their paths or throwing why one did not get there. */
  async put(
    connection: ConnectionLike | undefined,
    files: readonly Attachment[],
    mark: (id: string, change: Partial<Attachment>) => void,
  ): Promise<Attachment[]> {
    let why = '';
    const sent = await uploadEach(
      files,
      async (attachment) => {
        if (!connection?.isOpen()) throw new Error(this.host.say('the host is not connected'));
        const bytes = await this.host.read(attachment.uri);
        const path = await connection.attachFile(this.agentId, attachment.name, attachment.mime, bytes);
        this.sentFiles = new Map([...this.sentFiles, [path, attachment]]);
        return path;
      },
      mark,
      (error) => {
        why ||= problemOf(error);
        return problemOf(error);
      },
    );
    if (!sent) throw new Error(why);
    return sent;
  }

  private show(files: readonly Attachment[]) {
    if (files.length) this.sentFiles = new Map([...this.sentFiles, ...files.map((file) => [shownPath(file), file] as const)]);
  }

  /** Sends a message; one that names agents, chats or issues goes as a call, since the phone's own request carries none. */
  private async prompt(connection: ConnectionLike, messageId: string, text: string, paths: string[], context: readonly ContextChip[]) {
    if (!context.length) return connection.prompt(this.agentId, messageId, text, paths);
    await callHost(connection, 'promptChat', { agentId: this.agentId, messageId, text, paths, context: [...context] });
  }

  private inOrder(work: () => Promise<void>): Promise<void> {
    const next = this.line.then(work);
    this.line = next.catch(() => {});
    return next;
  }

  private push(entry: Outgoing) {
    const connection = this.host.connection();
    const id = entry.messageId;
    const generation = this.epoch;
    let steered = false;
    this.inOrder(async () => {
      if (this.epoch !== generation) return;
      if (!connection?.isOpen()) throw new Error(this.host.say('the host is not connected'));
      const files = this.entries.find((outgoing) => outgoing.messageId === id)?.files ?? entry.files;
      const sent = await this.put(connection, files, (fileId, change) => {
        this.entries = this.entries.map((outgoing) =>
          outgoing.messageId === id
            ? { ...outgoing, files: outgoing.files.map((file) => (file.id === fileId ? { ...file, ...change } : file)) }
            : outgoing,
        );
      });
      if (this.epoch !== generation) return;
      const paths = [...pathsOf(sent), ...entry.paths];
      if (entry.steers && this.host.chat().running) {
        steered = true;
        // Clear disk copy before putting a non-deduplicable steer on the wire so a kill mid-send cannot double-steer.
        this.update(id, { kept: false });
        this.persistWaiting();
        const ids = this.onHostQueue(this.host.chat().queued.map((held) => held.id));
        const message = { text: entry.text, paths, context: entry.context };
        await callHost(connection, 'steerQueued', { agentId: this.agentId, ids, message });
      } else {
        await this.prompt(connection, id, entry.text, paths, entry.context);
      }
    })
      .then(() => {
        if (this.epoch !== generation) return;
        if (steered) {
          this.entries = this.entries.filter((outgoing) => outgoing.messageId !== id);
        } else {
          this.update(id, { state: 'sent', kept: false });
          this.settle();
        }
        this.persistWaiting();
        this.host.emit();
      })
      .catch((error: unknown) => {
        if (this.epoch !== generation) return;
        if (!entry.steers && !connection?.isOpen()) {
          this.update(id, { waiting: true, kept: true, state: 'sending', problem: undefined });
          this.persistWaiting();
          const current = this.host.connection();
          if (current && current !== connection && current.isOpen() && (this.host.attached?.() ?? true)) {
            this.pushWaiting();
          }
          return;
        }
        this.update(id, { state: 'failed', problem: problemOf(error) });
        this.persistWaiting();
      });
  }

  private update(messageId: string, change: Partial<Outgoing>) {
    this.entries = this.entries.map((outgoing) => (outgoing.messageId === messageId ? { ...outgoing, ...change } : outgoing));
    this.host.emit();
  }
}
