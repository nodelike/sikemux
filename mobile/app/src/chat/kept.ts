import { Directory, File, Paths } from 'expo-file-system';
import * as Updates from 'expo-updates';
import type { ChatRows } from '@shared/chat/rows';

import { writeWhole } from '@/devices/disk';
import type { StashItem } from '@/stash/api';
import type { ContextChip } from '@/stash/words';
import type { Attachment } from './uploads';

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
        } catch (error) {
          console.warn('sikemux: could not keep the chat on this phone', error);
        }
      }),
  };
}

/** What was written in a chat's composer here and not saved on the host yet, with the host's revision it was written over. */
type UnsavedDraft = { text: string; attachments: Attachment[]; chips?: ContextChip[]; extra: Record<string, unknown>; rev?: number };

export type DraftStore = { load: () => UnsavedDraft | undefined; save: (draft: UnsavedDraft) => void; clear: () => void };

/** Kept with the app's documents rather than its cache, so writing survives the app quitting before the host has it. */
const drafts = new Directory(Paths.document, 'drafts');

function documentFile<T>(name: string) {
  const file = new File(drafts, `${encodeURIComponent(name)}.json`);
  return {
    load: (): T | undefined => {
      try {
        return file.exists ? (JSON.parse(file.textSync()) as T) : undefined;
      } catch {
        return undefined;
      }
    },
    save: (value: T) => {
      try {
        drafts.create({ idempotent: true, intermediates: true });
        writeWhole(file, JSON.stringify(value));
      } catch (error) {
        console.warn('sikemux: could not keep the draft on this phone', error);
      }
    },
    clear: () => {
      try {
        if (file.exists) file.delete();
      } catch (error) {
        console.warn('sikemux: could not let go of a saved draft', error);
      }
    },
  };
}

export const unsavedDraft = (core: string, agentId: string): DraftStore => documentFile<UnsavedDraft>(`${core}:${agentId}`);

/** What the new-chat screen held when it was left, so coming back to it finds it as it was. */
export type NewChatDraft = { text: string; attachments: Attachment[]; chips: StashItem[] };

export const newChatDraft = (core: string) => documentFile<NewChatDraft>(`${core}:new`);
