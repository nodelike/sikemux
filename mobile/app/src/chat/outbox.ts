import type { ConnectionLike } from '@sikemux/native';
import { randomUUID } from 'expo-crypto';

import type { ChatRows } from '@shared/chat/rows';
import type { Say } from '@shared/lib/say';
import type { ChatMessage, QueuedMessage } from '@shared/wire.generated';

import { callHost } from '@/core/hostCalls';
import { problem as problemOf } from '@/core/problem';
import type { ContextChip } from '@/stash/words';
import type { Named } from './draft';
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
  connection: () => ConnectionLike | undefined;
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
  /** Messages reach the host in the order they were sent, however long their files take. */
  private line: Promise<void> = Promise.resolve();
  private overOf: { chat: ChatRows; entries: Outgoing[]; holding: Holding[]; view: ChatRows } | undefined;
  private unsentOf: { entries: Outgoing[]; unsent: ReadonlyMap<string, Unsent> } | undefined;

  constructor(
    private readonly agentId: string,
    private readonly host: Host,
  ) {}

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

  deliver(text: string, files: Attachment[], waiting = false, named: Named = NAMED_NONE) {
    this.show(files);
    const entry: Outgoing = { messageId: randomUUID(), text, files, ...named, at: this.host.now(), state: 'sending', waiting };
    this.entries = [...this.entries, entry];
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
    this.entries = this.entries.map((outgoing) => (outgoing.waiting ? { ...outgoing, waiting: false } : outgoing));
    waiting.forEach((entry) => this.push(entry));
  }

  failWaiting(problem: string) {
    if (!this.waiting()) return;
    this.entries = this.entries.map((outgoing) =>
      outgoing.waiting ? { ...outgoing, waiting: false, state: 'failed', problem } : outgoing,
    );
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
    return this.inOrder(async () => {
      if (!connection?.isOpen()) throw new Error(this.host.say('the host is not connected'));
      const sent = await this.put(connection, files, () => {});
      const paths = [...pathsOf(sent), ...named.paths];
      if (!steers) return this.prompt(connection, id, text, paths, named.context);
      const ids = this.onHostQueue(this.host.chat().queued.map((held) => held.id));
      const message = { text, paths, context: named.context };
      await callHost(connection, 'steerQueued', { agentId: this.agentId, ids, message });
    })
      .then(() => {
        const held = this.holding.find((holding) => holding.id === id);
        if (held && (steers || !notQueued([held], this.host.chat().queued).length))
          this.holding = this.holding.filter((holding) => holding !== held);
        else this.holding = this.holding.map((holding) => (holding.id === id ? { ...holding, taken: true } : holding));
        this.host.emit();
      })
      .catch((error: unknown) => {
        this.holding = this.holding.filter((holding) => holding.id !== id);
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
    if (!this.entries.some((outgoing) => outgoing.state === 'sent')) return;
    if (turnEnded) {
      this.entries = this.entries.filter((outgoing) => outgoing.state !== 'sent');
      return;
    }
    const chat = this.host.chat();
    const shown = new Set(chat.messages.flatMap((row) => (row.promptId ? [row.promptId] : [])));
    const kept = this.entries.filter(
      (outgoing) =>
        outgoing.state !== 'sent' || (!shown.has(outgoing.messageId) && !chat.queued.some((held) => held.id === outgoing.messageId)),
    );
    if (kept.length !== this.entries.length) this.entries = kept;
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
    this.inOrder(async () => {
      if (!connection?.isOpen()) throw new Error(this.host.say('the host is not connected'));
      const files = this.entries.find((outgoing) => outgoing.messageId === id)?.files ?? entry.files;
      const sent = await this.put(connection, files, (fileId, change) => {
        this.entries = this.entries.map((outgoing) =>
          outgoing.messageId === id
            ? { ...outgoing, files: outgoing.files.map((file) => (file.id === fileId ? { ...file, ...change } : file)) }
            : outgoing,
        );
      });
      await this.prompt(connection, id, entry.text, [...pathsOf(sent), ...entry.paths], entry.context);
    })
      .then(() => {
        this.update(id, { state: 'sent' });
        this.settle();
        this.host.emit();
      })
      .catch((error: unknown) => {
        this.update(id, { state: 'failed', problem: problemOf(error) });
      });
  }

  private update(messageId: string, change: Partial<Outgoing>) {
    this.entries = this.entries.map((outgoing) => (outgoing.messageId === messageId ? { ...outgoing, ...change } : outgoing));
    this.host.emit();
  }
}
