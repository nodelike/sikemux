import { useCallback } from 'react';
import { AppState } from 'react-native';
import { useFocusEffect } from 'expo-router';

import { callHost } from '@/core/hostCalls';
import { problem } from '@/core/problem';
import { connectionOf, liveOf } from '@/devices/hub';

/**
 * Tells each host which of its chats this phone shows, as the Mac reports its screen: a chat shown stays awake, and one
 * shown with the app in front and the phone unlocked counts as read, and for the chief as the person being with it.
 * With the app behind or the phone locked, the host hears an empty list.
 */
const shown = new Map<string, string>();
const sent = new Map<string, string>();
let inFront = AppState.currentState === 'active';

AppState.addEventListener('change', (state) => {
  inFront = state === 'active';
  for (const core of new Set([...shown.keys(), ...sent.keys()])) tell(core);
});

function tell(core: string) {
  const agentId = inFront ? shown.get(core) : undefined;
  const params = { agentIds: agentId ? [agentId] : [], focused: !!agentId };
  const key = JSON.stringify(params);
  if (sent.get(core) === key) return;
  const connection = connectionOf(liveOf(core));
  if (!connection) return;
  sent.set(core, key);
  callHost(connection, 'onScreen', params).catch((error: unknown) => {
    sent.delete(core);
    console.warn('sikemux: the host did not take what this phone shows', problem(error));
  });
}

/** Reports chat `agentId` on screen while its screen is in front, on a phone with full access to the host. */
export function useOnScreen(core: string, agentId: string, full: boolean) {
  useFocusEffect(
    useCallback(() => {
      if (!full) return;
      shown.set(core, agentId);
      queueMicrotask(() => tell(core));
      return () => {
        if (shown.get(core) === agentId) shown.delete(core);
        queueMicrotask(() => tell(core));
      };
    }, [core, agentId, full]),
  );
}
