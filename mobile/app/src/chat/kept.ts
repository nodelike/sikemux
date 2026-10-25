import { Directory, File, Paths } from 'expo-file-system';
import * as Updates from 'expo-updates';
import type { ChatRows } from '@shared/chat/rows';

import { writeWhole } from '@/devices/disk';

/** A chat as this phone last drew it, so opening it again draws at once and asks the host only for what came after. */
export type Kept = {
  chat: ChatRows;
  mark: { feed: string; seq: string };
};

export type ChatStore = { load: () => Kept | undefined; save: (kept: Kept) => void };

type Stored = Kept & { build: string };

/** Past this many chats, the one written down longest ago is let go. */
const MAX_CHATS = 40;
/** A chat drawn bigger than this is opened from the host instead. */
const MAX_BYTES = 2 * 1024 * 1024;

// Another build of the app may draw a chat in a shape this one does not read.
const build = `${Updates.runtimeVersion ?? ''}/${Updates.updateId ?? 'embedded'}`;
const folder = new Directory(Paths.cache, 'chats');

function evictOldest() {
  const files = folder.list().filter((entry): entry is File => entry instanceof File && entry.uri.endsWith('.json'));
  if (files.length <= MAX_CHATS) return;
  const age = (file: File) => file.lastModified ?? 0;
  for (const file of files.sort((a, b) => age(a) - age(b)).slice(0, files.length - MAX_CHATS)) file.delete();
}

export function keptChat(core: string, agentId: string): ChatStore {
  const file = new File(folder, `${encodeURIComponent(`${core}:${agentId}`)}.json`);
  return {
    load: () => {
      try {
        if (!file.exists) return undefined;
        const stored = JSON.parse(file.textSync()) as Stored;
        return stored.build === build ? stored : undefined;
      } catch {
        return undefined;
      }
    },
    // After the current frame, so leaving a chat never waits on writing it down.
    save: (kept) =>
      setTimeout(() => {
        try {
          const text = JSON.stringify({ ...kept, build } satisfies Stored);
          if (text.length > MAX_BYTES) return;
          folder.create({ idempotent: true, intermediates: true });
          writeWhole(file, text);
          evictOldest();
        } catch {}
      }),
  };
}
