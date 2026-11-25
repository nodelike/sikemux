import { Directory, File, Paths } from 'expo-file-system';

import type { Snapshot } from '@/core/protocol';
import { writeWhole } from './disk';

const MAX_BYTES = 4 * 1024 * 1024;
const SAVE_DELAY_MS = 2000;

const folder = new Directory(Paths.cache, 'views');
const pending = new Map<string, { snapshot: Snapshot; timer: ReturnType<typeof setTimeout> }>();

function fileOf(core: string): File {
  return new File(folder, `${encodeURIComponent(core)}.json`);
}

function isSnapshot(value: unknown): value is Snapshot {
  if (!value || typeof value !== 'object') return false;
  const view = value as Partial<Snapshot>;
  return (
    !!view.workspace &&
    typeof view.workspace === 'object' &&
    Array.isArray(view.chats) &&
    Array.isArray(view.sessions) &&
    !!view.inbox &&
    typeof view.inbox === 'object'
  );
}

function writeNow(core: string, snapshot: Snapshot) {
  try {
    const text = JSON.stringify(snapshot);
    if (text.length > MAX_BYTES) return;
    folder.create({ idempotent: true, intermediates: true });
    writeWhole(fileOf(core), text);
  } catch (error) {
    console.warn('sikemux: could not keep the host view on this phone', error);
  }
}

export async function loadKeptView(core: string): Promise<Snapshot | undefined> {
  try {
    const file = fileOf(core);
    if (!file.exists) return undefined;
    const parsed = JSON.parse(await file.text()) as unknown;
    return isSnapshot(parsed) ? parsed : undefined;
  } catch {
    return undefined;
  }
}

export function saveKeptView(core: string, snapshot: Snapshot) {
  const queued = pending.get(core);
  if (queued) {
    queued.snapshot = snapshot;
    return;
  }
  const timer = setTimeout(() => {
    const entry = pending.get(core);
    if (!entry) return;
    pending.delete(core);
    writeNow(core, entry.snapshot);
  }, SAVE_DELAY_MS);
  pending.set(core, { snapshot, timer });
}

export function flushKeptViews() {
  pending.forEach(({ snapshot, timer }, core) => {
    clearTimeout(timer);
    writeNow(core, snapshot);
  });
  pending.clear();
}

export function forgetKeptView(core: string) {
  const queued = pending.get(core);
  if (queued) {
    clearTimeout(queued.timer);
    pending.delete(core);
  }
  try {
    const file = fileOf(core);
    if (file.exists) file.delete();
  } catch (error) {
    console.warn('sikemux: could not let go of a kept host view', error);
  }
}
