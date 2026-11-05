import { useEffect } from 'react';
import * as SecureStore from 'expo-secure-store';

import { externalValue, useExternalValue } from '@shared/lib/externalValue';

/** Whether the person wants notifications: undefined until they first choose. */
type Choice = 'on' | 'off' | undefined;

const ITEM = 'sikemux.notifications';

const choice = externalValue<Choice>(undefined);
let loaded: Promise<Choice> | undefined;
const offered = externalValue(false);

export function notificationsChoice(): Promise<Choice> {
  loaded ??= SecureStore.getItemAsync(ITEM).then((stored) => {
    choice.set(stored === 'on' || stored === 'off' ? stored : undefined);
    return choice.get();
  });
  return loaded;
}

export async function choose(next: Choice) {
  loaded = Promise.resolve(next);
  if (next) await SecureStore.setItemAsync(ITEM, next);
  else await SecureStore.deleteItemAsync(ITEM);
  choice.set(next);
}

export function useNotificationsChoice(): Choice {
  useEffect(() => {
    notificationsChoice().catch((error: unknown) => console.warn('sikemux: could not read the notifications choice', error));
  }, []);
  return useExternalValue(choice);
}

/** Asks once, after a host lets the phone in, whether to turn notifications on; a person who already chose is not asked. */
export async function offerNotifications() {
  if ((await notificationsChoice()) !== undefined) return;
  offered.set(true);
}

export function closeOffer() {
  offered.set(false);
}

export function useOffered(): boolean {
  return useExternalValue(offered);
}
