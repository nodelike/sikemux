import { useEffect } from 'react';
import { router } from 'expo-router';

import { stashNative, type Shared } from '../../modules/stash';

let waiting: Shared | null = null;

/** What the share screen shows, once. */
export function takeIncoming(): Shared | null {
  const shared = waiting;
  waiting = null;
  return shared;
}

/** Opens the share screen whenever another app shares to Sikemux, including the share that opened the app. */
export function useShareIntake() {
  useEffect(() => {
    const native = stashNative;
    const take = native?.take;
    if (!native || !take) return;
    const look = () =>
      take()
        .then((shared) => {
          if (!shared || (!shared.text?.trim() && !shared.files.length)) return;
          waiting = shared;
          router.push('/stash');
        })
        .catch((error: unknown) => console.warn('sikemux: could not take what was shared to the stash', error));
    look();
    const listening = native.addListener?.('onShare', look);
    return () => listening?.remove();
  }, []);
}

export function leaveToSharer() {
  stashNative?.leave?.();
}
