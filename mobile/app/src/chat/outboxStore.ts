import { Directory, File, Paths } from 'expo-file-system';

import { writeWhole } from '@/devices/disk';
import type { ContextChip } from '@/stash/words';
import type { Attachment } from './uploads';

/** A message sent while offline/reconnecting, kept in documents so it outlives kills and OTA reloads. */
export type SavedOutboxEntry = {
  messageId: string;
  text: string;
  files: Attachment[];
  paths: string[];
  context: ContextChip[];
  at: number;
  steers?: boolean;
  state?: 'waiting' | 'failed';
  problem?: string;
};

export type OutboxStore = {
  load: () => SavedOutboxEntry[];
  save: (entries: SavedOutboxEntry[]) => void;
  clear: () => void;
};

const outboxes = new Directory(Paths.document, 'outbox');

export function waitingOutbox(core: string, agentId: string): OutboxStore {
  const file = new File(outboxes, `${encodeURIComponent(`${core}:${agentId}`)}.json`);
  return {
    load: () => {
      try {
        if (!file.exists) return [];
        const parsed = JSON.parse(file.textSync()) as unknown;
        return Array.isArray(parsed) ? (parsed as SavedOutboxEntry[]) : [];
      } catch {
        return [];
      }
    },
    save: (entries) => {
      if (!entries.length) {
        try {
          if (file.exists) file.delete();
        } catch {}
        return;
      }
      try {
        outboxes.create({ idempotent: true, intermediates: true });
        writeWhole(file, JSON.stringify(entries));
      } catch (error) {
        console.warn('sikemux: could not keep waiting messages on this phone', error);
      }
    },
    clear: () => {
      try {
        if (file.exists) file.delete();
      } catch {}
    },
  };
}

/** Chats that hold saved waiting messages from before a kill or reload. */
export function listPendingOutbox(): { core: string; agentId: string }[] {
  try {
    if (!outboxes.exists) return [];
    const found: { core: string; agentId: string }[] = [];
    for (const entry of outboxes.list()) {
      if (!(entry instanceof File) || !entry.uri.endsWith('.json')) continue;
      const base = decodeURIComponent(entry.name.slice(0, -'.json'.length));
      const sep = base.indexOf(':');
      if (sep <= 0) continue;
      found.push({ core: base.slice(0, sep), agentId: base.slice(sep + 1) });
    }
    return found;
  } catch {
    return [];
  }
}

/** Deletes all saved waiting messages for `core` (e.g. when the host is forgotten or unpaired). */
export function forgetOutboxForHost(core: string) {
  try {
    if (!outboxes.exists) return;
    const prefix = `${core}:`;
    for (const entry of outboxes.list()) {
      if (!(entry instanceof File) || !entry.uri.endsWith('.json')) continue;
      const base = decodeURIComponent(entry.name.slice(0, -'.json'.length));
      if (base.startsWith(prefix)) {
        try {
          entry.delete();
        } catch {}
      }
    }
  } catch {}
}
