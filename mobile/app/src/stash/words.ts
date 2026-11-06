import { STASH_URI } from '@shared/wire.generated';
import { formatAge, formatBytes } from '@shared/lib/format';

import type { StashItem } from './api';

export function siteOf(url: string): string {
  return url.replace(/^[a-z]+:\/\//i, '').split(/[/?#]/)[0] ?? url;
}

/** What an item is called in a list: a page's title, a text's or prompt's first line, or a file's name. */
export function itemTitle(item: StashItem): string {
  if (item.kind === 'link') return item.preview?.title || (item.url ?? '').replace(/^https?:\/\//, '');
  if (item.kind === 'text' || item.kind === 'prompt') return (item.text ?? '').trim().split('\n')[0] ?? '';
  return item.name ?? 'file';
}

/** The line under it: where a link goes or what the item is, then how long ago it was stashed. */
export function itemPlace(item: StashItem): string {
  const what =
    item.kind === 'link'
      ? item.preview?.site || siteOf(item.url ?? '')
      : item.kind === 'text'
        ? 'Text'
        : item.kind === 'prompt'
          ? promptWords(item)
          : formatBytes(item.size ?? 0);
  const age = formatAge(item.createdAt);
  return `${what} · ${age === 'now' ? 'just now' : age}`;
}

function promptWords(item: StashItem): string {
  const files = item.attachments?.length ?? 0;
  return files ? `Prompt · ${files} ${files === 1 ? 'file' : 'files'}` : 'Prompt';
}

export const carriesFile = (item: StashItem) => item.kind === 'image' || item.kind === 'video' || item.kind === 'file';

const MENTION = /(^|\s)&(\S*)$/;

/** What follows an `&` that starts the last word of `draft`, or undefined when there is none. */
export function stashQuery(draft: string): string | undefined {
  return MENTION.exec(draft)?.[2];
}

/** `draft` without the `&` word it ends with. */
export function withoutQuery(draft: string): string {
  return draft.replace(MENTION, '$1').trimEnd();
}

/** The items whose title, note, link or text holds every word of `query`, ignoring case. */
export function stashMatches(items: readonly StashItem[], query: string): StashItem[] {
  const words = query
    .toLowerCase()
    .split(/[\s&]+/)
    .filter(Boolean);
  return items.filter((item) => {
    const said = [itemTitle(item), item.note, item.url, item.text, item.preview?.site].filter(Boolean).join(' ').toLowerCase();
    return words.every((word) => said.includes(word));
  });
}

/** A stashed link or text as a chat's draft holds it, in `extra.context`: a chip in every app, sent with the message. */
export type ContextChip = { uri: string; title: string; text: string };

/** How the host puts a link or text in a draft, for a draft that is still only on this phone. */
export function contextChip(item: StashItem): ContextChip {
  if (item.kind === 'link') {
    const url = item.url ?? '';
    const title = item.preview?.title || url;
    return { uri: url, title, text: [title, url, item.preview?.description].filter(Boolean).join('\n\n') };
  }
  return { uri: `${STASH_URI}${item.id}`, title: itemTitle(item).slice(0, 80), text: item.text ?? '' };
}

/** The chips a draft's `extra.context` holds, leaving out any it cannot read. */
export function contextChipsOf(value: unknown): ContextChip[] {
  if (!Array.isArray(value)) return [];
  return value.filter(
    (chip): chip is ContextChip =>
      typeof chip === 'object' &&
      chip !== null &&
      typeof chip.uri === 'string' &&
      typeof chip.title === 'string' &&
      typeof chip.text === 'string',
  );
}

/** The message as this phone sends it: what was written, then each chip's words. */
export function withContext(text: string, chips: readonly ContextChip[]): string {
  return [text, ...chips.map((chip) => chip.text)].filter(Boolean).join('\n\n');
}
