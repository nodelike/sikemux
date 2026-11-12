import { useCallback } from 'react';
import { AppState } from 'react-native';
import { useFocusEffect } from 'expo-router';

import type { ConnectionLike } from '@sikemux/native';
import { callHost } from '@/core/hostCalls';
import { problem } from '@/core/problem';
import { connectionOf, liveOf } from '@/devices/hub';
import { lockOpen, onLockChange } from '@/lock/lock';

/**
 * Tells each host which of its chats this phone shows, as the Mac reports its screen: a chat shown stays awake, and one
 * shown live, the app in front and nothing locking it counts as read, and for the chief as the person
 * being with it. With the app behind or locked, the host hears an empty list.
 */
const shown = new Map<string, { agentId: string; drawn: boolean }>();
const sent = new Map<string, { to: ConnectionLike; said: string }>();
let inFront = AppState.currentState === 'active';

const tellAll = () => {
  for (const core of new Set([...shown.keys(), ...sent.keys()])) tell(core);
};
AppState.addEventListener('change', (state) => {
  inFront = state === 'active';
  tellAll();
});
onLockChange(tellAll);

function tell(core: string) {
  const connection = connectionOf(liveOf(core));
  if (!connection) return;
  const looking = inFront && lockOpen();
  const chat = looking ? shown.get(core) : undefined;
  const params = { agentIds: chat ? [chat.agentId] : [], focused: !!chat?.drawn };
  const said = JSON.stringify(params);
  const last = sent.get(core);
  // A new connection is a new client to the host, which has heard nothing from it yet.
  if (last?.to === connection && last.said === said) return;
  sent.set(core, { to: connection, said });
  callHost(connection, 'onScreen', params).catch((error: unknown) => {
    sent.delete(core);
    console.warn('sikemux: the host did not take what this phone shows', problem(error));
  });
}

/**
 * Reports chat `agentId` on screen while its screen is in front, on a phone with full access, and as read once it is
 * `drawn` live. Pass the connection so a new one hears it too.
 */
export function useOnScreen(core: string, agentId: string, full: boolean, drawn: boolean, connection: ConnectionLike | undefined) {
  useFocusEffect(
    useCallback(() => {
      if (!full || !connection) return;
      shown.set(core, { agentId, drawn });
      queueMicrotask(() => tell(core));
      return () => {
        if (shown.get(core)?.agentId === agentId) shown.delete(core);
        queueMicrotask(() => tell(core));
      };
    }, [core, agentId, full, drawn, connection]),
  );
}
