import { useEffect, useSyncExternalStore } from 'react';
import { AppState } from 'react-native';

import type { ConnectionLike } from '@sikemux/native';

import { connectionOf, liveOf, onAnyHost, onChatEvents, onHostForgotten, useLive, watch } from '@/devices/hub';
import { translate } from '@/i18n';
import { keptChat, listPendingOutbox, unsavedDraft, waitingOutbox } from './kept';
import { ChatSession, type ChatSnapshot, type OpenTimes } from './session';

function logOpen(agentId: string, took: OpenTimes) {
  const ms = (value: number) => `${Math.round(value)}ms`;
  const restored = took.restore === null ? 'nothing kept' : `kept copy read in ${ms(took.restore)}`;
  console.info(
    `sikemux: ${agentId} ${restored}, ${took.woke ? 'woke and ' : ''}${took.answer} after ${ms(took.answered)} with ${Math.round(took.bytes / 1024)} KB, applied by ${ms(took.applied)}, drawn by ${ms(took.drawn)}`,
  );
}

/** Every chat this phone opened since it started, capped to avoid unbounded memory growth. */
const MAX_INACTIVE_SESSIONS = 8;
const sessions = new Map<string, ChatSession>();
/** Sessions that hold messages waiting to send even after their chat screen closed. */
const pendingDelivery = new Map<string, { core: string; session: ChatSession; unwatch: () => void; release?: () => void }>();
let offHostWatcher: (() => void) | undefined;
let scannedSavedOutbox = false;

function evictInactiveSessions() {
  const inactive: [string, ChatSession][] = [];
  sessions.forEach((session, key) => {
    if (!session.isHeld() && !session.hasListeners() && !pendingDelivery.has(key) && !session.hasWaiting()) {
      inactive.push([key, session]);
    }
  });
  let excess = inactive.length - MAX_INACTIVE_SESSIONS;
  for (let i = 0; i < excess; i += 1) {
    const [key, session] = inactive[i];
    session.keep();
    session.keepDraft();
    sessions.delete(key);
  }
}

onHostForgotten((core) => {
  const prefix = `${core}:`;
  pendingDelivery.forEach((item, key) => {
    if (item.core !== core) return;
    item.release?.();
    item.unwatch();
    pendingDelivery.delete(key);
  });
  sessions.forEach((session, key) => {
    if (!key.startsWith(prefix)) return;
    session.clearOutbox();
    sessions.delete(key);
  });
  if (!pendingDelivery.size && offHostWatcher) {
    offHostWatcher();
    offHostWatcher = undefined;
  }
});

function flushPendingSessions() {
  pendingDelivery.forEach((item, key) => {
    if (!item.session.hasWaiting()) {
      item.release?.();
      item.unwatch();
      pendingDelivery.delete(key);
      return;
    }
    const connection = connectionOf(liveOf(item.core));
    if (connection && !item.release) {
      const info = liveOf(item.core).snapshot?.chats.find((chat) => chat.agentId === item.session.agentId);
      if (liveOf(item.core).snapshot) item.session.hostSaw(info);
      item.release = item.session.hold(connection);
    } else if (!connection && item.release) {
      item.release();
      item.release = undefined;
    }
  });
  if (!pendingDelivery.size && offHostWatcher) {
    offHostWatcher();
    offHostWatcher = undefined;
  }
}

function trackWaiting(core: string, agentId: string, session: ChatSession, waiting: boolean) {
  const key = `${core}:${agentId}`;
  if (waiting) {
    if (!pendingDelivery.has(key)) pendingDelivery.set(key, { core, session, unwatch: watch(core) });
    if (!offHostWatcher) offHostWatcher = onAnyHost(flushPendingSessions);
    flushPendingSessions();
  } else {
    const existing = pendingDelivery.get(key);
    if (existing) {
      existing.release?.();
      existing.unwatch();
      pendingDelivery.delete(key);
    }
    if (!pendingDelivery.size && offHostWatcher) {
      offHostWatcher();
      offHostWatcher = undefined;
    }
  }
}

/** Restores any chat with waiting messages saved on disk before a kill or OTA reload. */
export function resumePendingOutboxes() {
  if (scannedSavedOutbox) return;
  scannedSavedOutbox = true;
  for (const { core, agentId } of listPendingOutbox()) {
    const session = chatSession(core, agentId);
    if (session.hasWaiting()) trackWaiting(core, agentId, session, true);
  }
}

export function chatSession(core: string, agentId: string): ChatSession {
  const key = `${core}:${agentId}`;
  let session = sessions.get(key);
  if (session) {
    sessions.delete(key);
    sessions.set(key, session);
    return session;
  }
  session = new ChatSession(agentId, {
    listen: (take) => onChatEvents(core, take),
    store: keptChat(core, agentId),
    drafts: unsavedDraft(core, agentId),
    outbox: waitingOutbox(core, agentId),
    onWaiting: (waiting) => trackWaiting(core, agentId, session!, waiting),
    timed: __DEV__ ? (took) => logOpen(agentId, took) : undefined,
    say: translate,
  });
  sessions.set(key, session);
  if (session.hasWaiting()) trackWaiting(core, agentId, session, true);
  evictInactiveSessions();
  return session;
}

/** One chat on one host, attached while this is mounted and the host is connected. */
export function useChat(
  core: string,
  agentId: string,
): { session: ChatSession; chat: ChatSnapshot; connection: ConnectionLike | undefined } {
  const live = useLive(core);
  const session = chatSession(core, agentId);
  const connection = connectionOf(live);
  const snapshot = live.snapshot;
  const info = snapshot?.chats.find((chat) => chat.agentId === agentId);
  // Before the hold, so the first attach knows whether the chat needs waking.
  useEffect(() => {
    if (snapshot) session.hostSaw(info);
  }, [session, snapshot, info]);
  useEffect(() => {
    const release = session.hold(connection);
    return () => {
      release();
      evictInactiveSessions();
    };
  }, [session, connection]);
  // The phone may be closed for good once it leaves the app, with the chat still on screen.
  useEffect(() => {
    const watching = AppState.addEventListener('change', (state) => {
      if (state !== 'background') return;
      session.keep();
      session.keepDraft();
    });
    return () => watching.remove();
  }, [session]);

  const chat = useSyncExternalStore(session.subscribe, session.snapshot);
  return { session, chat, connection };
}

export function useDraft(session: ChatSession): string {
  return useSyncExternalStore(session.subscribe, session.draftText);
}

export function useDraftTokens(session: ChatSession): ChatSnapshot['tokens'] {
  return useSyncExternalStore(session.subscribe, () => session.snapshot().tokens);
}
