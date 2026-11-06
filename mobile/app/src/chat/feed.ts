import { MobileError, type ChatMark, type ConnectionLike } from '@sikemux/native';

import { applyChanges, emptyChat, fromView, withPage, type ChatRows } from '@shared/chat/rows';
import type { ChatChange, ChatPage, ChatView, ToolDetail } from '@shared/chat/types';

import { callHost } from '@/core/hostCalls';
import type { ChatDelivery } from '@/devices/hub';
import type { ChatStore, Kept } from './kept';

/** Turns of earlier rows asked for at a time. */
const PAGE_TURNS = 20;

/** Finished calls whose change and output are kept once read, so a row opened again shows them at once. */
const KEPT_DETAILS = 40;

/** Whether a delivery that catches this phone up after it fell behind starts from `taken`. */
function follows(changes: readonly ChatChange[], taken: bigint): boolean {
  const first = changes[0];
  return first?.op === 'head' && first.follows !== undefined && BigInt(first.follows) === taken;
}

function animationFrame(run: () => void): () => void {
  const frame = requestAnimationFrame(run);
  return () => cancelAnimationFrame(frame);
}

type Hooks = {
  /** Runs once before the next frame is drawn; returns how to call that off. */
  frame?: (run: () => void) => () => void;
  store?: ChatStore;
  /** The chat changed from `before`. */
  applied: (before: ChatRows) => void;
  emit: () => void;
  /** Some changes never arrived, so the chat must be attached again from what was taken. */
  fellBehind: () => void;
};

/** The chat as the host folds it, and where its changes got to. */
export class Feed {
  chat: ChatRows = emptyChat;
  /** The last change the chat holds; a reconnect asks only for what comes after it. */
  mark: ChatMark | undefined;
  earlier: 'idle' | 'loading' | 'failed' = 'idle';
  /** The last change taken, applied or not yet. */
  private taken: bigint | undefined;
  private pending: ChatChange[] = [];
  private cancelFrame: (() => void) | undefined;
  private paging: Promise<boolean> | undefined;
  /** Counts cuts, so a page that comes back after the chat was taken up again is dropped. */
  private generation = 0;
  private details = new Map<string, Promise<ToolDetail>>();
  private keptChat: ChatRows | undefined;

  constructor(private readonly hooks: Hooks) {}

  get frame() {
    return this.hooks.frame ?? animationFrame;
  }

  /** Draws the chat as this phone last kept it; the first attach asks only for what came after. */
  restore(kept: Kept) {
    this.chat = kept.chat;
    this.keptChat = kept.chat;
    this.mark = { feed: kept.mark.feed, seq: BigInt(kept.mark.seq) };
    this.taken = this.mark.seq;
  }

  /** Writes the chat down as drawn, unless it has not changed since it last was. */
  keep() {
    const store = this.hooks.store;
    const mark = this.mark;
    if (!store || !mark || this.chat === this.keptChat) return;
    this.keptChat = this.chat;
    store.save({ chat: this.chat, mark: { feed: mark.feed, seq: String(mark.seq) } });
  }

  /** Forgets where the chat got to, so the next attach reads it whole. */
  forget() {
    this.mark = undefined;
    this.taken = undefined;
  }

  /** Lets go of what the last attach had under way. */
  cut() {
    this.generation += 1;
    if (this.earlier === 'loading') this.earlier = 'idle';
    this.paging = undefined;
  }

  /** Lays changes over the chat. */
  apply(change: (chat: ChatRows) => ChatRows) {
    const before = this.chat;
    this.chat = change(before);
    this.hooks.applied(before);
  }

  /** Starts again from the host's whole view, as of `mark`. */
  openAt(view: ChatView, mark: ChatMark) {
    this.pending = [];
    this.apply(() => fromView(view));
    this.attachedAt(mark);
  }

  /** Catches up with what the host sent since the mark this phone held, up to `mark`. */
  resumeAt(changes: ChatChange[], mark: ChatMark) {
    this.apply((chat) => applyChanges(chat, changes));
    this.attachedAt(mark);
  }

  private attachedAt(mark: ChatMark) {
    this.mark = mark;
    this.taken = mark.seq;
  }

  take(deliveries: ChatDelivery[]) {
    for (const delivery of deliveries) {
      const taken = this.taken;
      if (taken !== undefined && delivery.seq <= taken) continue;
      const changes = JSON.parse(delivery.changesJson) as ChatChange[];
      if (taken !== undefined && delivery.seq !== taken + 1n && !follows(changes, taken)) {
        this.hooks.fellBehind();
        return;
      }
      this.pending.push(...changes);
      this.taken = delivery.seq;
    }
    // A stream of tokens is drawn once a frame, not once a change.
    this.cancelFrame ??= this.frame(this.flush);
  }

  private flush = () => {
    this.applyPending();
  };

  /** Applies what was taken, and only then moves the mark past it. */
  applyPending() {
    this.cancelFrame?.();
    this.cancelFrame = undefined;
    const advanced = this.mark && this.taken !== undefined && this.taken !== this.mark.seq;
    if (!this.pending.length && !advanced) return;
    const batch = this.pending;
    this.pending = [];
    if (batch.length) this.apply((chat) => applyChanges(chat, batch));
    if (this.mark && this.taken !== undefined) this.mark = { ...this.mark, seq: this.taken };
    this.hooks.emit();
  }

  /** Answers whether earlier rows came; a fetch already under way is waited on rather than asked again. */
  pageBack(connection: ConnectionLike | undefined, agentId: string): Promise<boolean> {
    if (this.paging) return this.paging;
    const before = this.chat.olderBefore;
    const feed = this.mark?.feed;
    if (before === null || feed === undefined || !connection) return Promise.resolve(false);
    this.earlier = 'loading';
    this.hooks.emit();
    const generation = this.generation;
    const paging = connection
      .chatHistory(agentId, feed, BigInt(before), PAGE_TURNS)
      .then((json) => {
        if (generation !== this.generation) return false;
        this.chat = withPage(this.chat, JSON.parse(json) as ChatPage);
        this.earlier = 'idle';
        return true;
      })
      .catch((error: unknown) => {
        if (generation !== this.generation) return false;
        if (MobileError.Refused.instanceOf(error)) {
          this.chat = { ...this.chat, olderBefore: null };
          this.earlier = 'idle';
        } else {
          this.earlier = 'failed';
        }
        return false;
      })
      .finally(() => {
        if (this.paging === paging) this.paging = undefined;
        this.hooks.emit();
      });
    this.paging = paging;
    return paging;
  }

  /** A call's whole change and what it printed, which rows carry only counts of. `done` calls are read once. */
  toolDetail(connection: ConnectionLike | undefined, agentId: string, toolCallId: string, done: boolean): Promise<ToolDetail> {
    const kept = this.details.get(toolCallId);
    if (kept) return kept;
    const reading = Promise.resolve().then(() => {
      if (!connection?.isOpen()) throw new Error('the host is not connected');
      return callHost(connection, 'chatToolDetail', { agentId, toolCallId });
    });
    if (!done) return reading;
    this.details.set(toolCallId, reading);
    reading.catch(() => this.details.delete(toolCallId));
    if (this.details.size > KEPT_DETAILS) this.details.delete(this.details.keys().next().value as string);
    return reading;
  }
}
