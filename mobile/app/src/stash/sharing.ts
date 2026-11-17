import { File, Paths } from 'expo-file-system';
import type { StashAdd } from '@sikemux/protocol';
import { MAX_STASH_FILE_BYTES, MAX_STASH_TEXT } from '@sikemux/protocol/stash';

import type { Shared, SharedFile } from '../../modules/stash';
import { api, type TokenSource } from '@/account/api';
import { stashFile, type StashItem, type StashTarget } from './api';

export type Outgoing = { kind: 'link'; url: string; title?: string } | { kind: 'text'; text: string } | { kind: 'file'; file: SharedFile };

const LINK = /https?:\/\/[^\s<>"]+/i;
/** Words around a link up to this long read as its title, as a browser shares "Title https://…". */
const TITLE_MOST = 300;

/** What was shared, as the items it stashes: each file, then the text, or the link in it. */
export function outgoingOf(shared: Shared): Outgoing[] {
  const items: Outgoing[] = shared.files.map((file) => ({ kind: 'file', file }));
  const text = shared.text?.trim();
  if (!text) return items;
  const link = LINK.exec(text)?.[0];
  const around = link ? text.replace(link, '').trim() : '';
  if (link && around.length <= TITLE_MOST) {
    items.push({ kind: 'link', url: link, title: shared.subject?.trim() || around || undefined });
  } else if (!shared.files.length || text !== shared.subject?.trim()) {
    items.push({ kind: 'text', text });
  }
  return items;
}

export function tooBig(item: Outgoing): boolean {
  return item.kind === 'file' && (!item.file.uri || item.file.size > MAX_STASH_FILE_BYTES);
}

/** Text longer than the server takes as text goes as a text file. */
function asFile(text: string): SharedFile {
  const file = new File(Paths.cache, `shared-${Date.now()}.txt`);
  file.write(text);
  return { uri: file.uri, name: 'Shared text.txt', mime: 'text/plain', size: file.size ?? text.length };
}

/** Stashes each item in turn, with the note and target on every one, reporting how much of it all has gone. */
export async function stashAll(
  token: TokenSource,
  items: readonly Outgoing[],
  extra: { note?: string; target?: StashTarget },
  onProgress: (share: number) => void,
  signal?: AbortSignal,
): Promise<StashItem[]> {
  const weights = items.map((item) => (item.kind === 'file' ? Math.max(item.file.size, 1) : 1));
  const total = weights.reduce((sum, weight) => sum + weight, 0);
  let done = 0;
  const stashed: StashItem[] = [];
  for (const [index, item] of items.entries()) {
    const weight = weights[index];
    const before = done;
    const file = item.kind === 'file' ? item.file : item.kind === 'text' && item.text.length > MAX_STASH_TEXT ? asFile(item.text) : null;
    if (file?.uri) {
      stashed.push(
        await stashFile(token, { uri: file.uri, name: file.name, mime: file.mime }, extra, {
          signal,
          onProgress: (share) => onProgress((before + share * weight) / total),
        }),
      );
    } else if (item.kind !== 'file') {
      const body: StashAdd = item.kind === 'link' ? { kind: 'link', url: item.url, ...extra } : { kind: 'text', text: item.text, ...extra };
      stashed.push(await api(token).addStash(body));
    }
    done += weight;
    onProgress(done / total);
  }
  return stashed;
}
