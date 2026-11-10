import { useEffect } from 'react';
import * as SecureStore from 'expo-secure-store';

import { externalValue, useExternalValue } from '@shared/lib/externalValue';

/** The agent list's groups folded shut, by key, as the Mac's rail keeps its own. */
const ITEM = 'sikemux.folded-agent-groups';

const folded = externalValue<string[]>(['settled']);
let loaded: Promise<void> | undefined;

function load() {
  loaded ??= SecureStore.getItemAsync(ITEM).then((stored) => {
    if (stored) folded.set(JSON.parse(stored) as string[]);
  });
  return loaded;
}

export function toggleFolded(key: string) {
  const now = folded.get();
  const next = now.includes(key) ? now.filter((shut) => shut !== key) : [...now, key];
  folded.set(next);
  SecureStore.setItemAsync(ITEM, JSON.stringify(next)).catch((error: unknown) =>
    console.warn('sikemux: could not keep the folded groups', error),
  );
}

export function useFolded(key: string): boolean {
  useEffect(() => {
    load().catch((error: unknown) => console.warn('sikemux: could not read the folded groups', error));
  }, []);
  return useExternalValue(folded, (shut) => shut.includes(key));
}
