import { useEffect, useSyncExternalStore } from 'react';
import { AppState } from 'react-native';

import type { ConnectionLike } from '@sikemux/native';

import { connectionOf, onChatEvents, useLive } from '@/devices/hub';
import { keptChat } from './kept';
import { ChatSession, type ChatSnapshot, type OpenTimes } from './session';

function logOpen(agentId: string, took: OpenTimes) {
  const ms = (value: number) => `${Math.round(value)}ms`;
  const restored = took.restore === null ? 'nothing kept' : `kept copy read in ${ms(took.restore)}`;
  console.info(
    `sikemux: ${agentId} ${restored}, ${took.woke ? 'woke and ' : ''}${took.answer} after ${ms(took.answered)} with ${Math.round(took.bytes / 1024)} KB, applied by ${ms(took.applied)}, drawn by ${ms(took.drawn)}`,
  );
}

/** Every chat this phone opened since it started, so going back to one finds it as it was left. */
const sessions = new Map<string, ChatSession>();

export function chatSession(core: string, agentId: string): ChatSession {
  const key = `${core}:${agentId}`;
  let session = sessions.get(key);
  if (!session) {
    session = new ChatSession(agentId, {
      listen: (take) => onChatEvents(core, take),
      store: keptChat(core, agentId),
      timed: __DEV__ ? (took) => logOpen(agentId, took) : undefined,
    });
    sessions.set(key, session);
  }
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
    if (!snapshot) return;
    const waiting = snapshot.attentions.filter((attention) => attention.agentId === agentId).map((attention) => attention.id);
    session.hostSaw(info, waiting);
  }, [session, snapshot, info, agentId]);
  useEffect(() => session.hold(connection), [session, connection]);
  // The phone may be closed for good once it leaves the app, with the chat still on screen.
  useEffect(() => {
    const watching = AppState.addEventListener('change', (state) => state === 'background' && session.keep());
    return () => watching.remove();
  }, [session]);

  const chat = useSyncExternalStore(session.subscribe, session.snapshot);
  return { session, chat, connection };
}

export function useDraft(session: ChatSession): string {
  return useSyncExternalStore(session.subscribe, session.draftText);
}
