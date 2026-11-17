import type { Attachment } from '@/chat/uploads';
import { translate } from '@/i18n';
import { fetchStashFile, type StashItem } from './api';
import { stashToken } from './store';
import { carriesFile } from './words';

/** A stash file brought down to this phone, for a chat that does not exist yet to take. */
export function downloadedFile(item: StashItem): Promise<Attachment> {
  const token = stashToken();
  if (!token) return Promise.reject(new Error(translate('Sign in again.')));
  return fetchStashFile(token, item);
}

/**
 * What picking stash items does to a message: each item's note joins its words, a link or text rides
 * beside them as a chip until it is sent, and `file` gets each file ready to go with it. A stashed
 * prompt comes back whole, its words into the message and its files beside them, and leaves the
 * stash at once, as `restored` lists.
 */
export function intoMessage(
  items: readonly StashItem[],
  draft: string,
  file: (item: StashItem) => Promise<Attachment>,
): { draft: string; chips: StashItem[]; files: Promise<Attachment[]> | null; restored: string[] } {
  const prompts = items.filter((item) => item.kind === 'prompt');
  const rest = items.filter((item) => item.kind !== 'prompt');
  const words = [
    ...prompts.map((prompt) => prompt.text?.trim() ?? ''),
    ...rest.flatMap((item) => (item.note?.trim() ? [item.note.trim()] : [])),
  ].filter(Boolean);
  const files = [
    ...rest.filter(carriesFile).map(file),
    ...prompts
      .flatMap((prompt) => prompt.attachments ?? [])
      .map((attached) => downloadedFile(attached).then((picked) => ({ ...picked, stashId: undefined }))),
  ];
  return {
    draft: words.length ? [draft.trimEnd(), ...words].filter(Boolean).join('\n\n') : draft,
    chips: rest.filter((item) => !carriesFile(item)),
    files: files.length ? Promise.all(files) : null,
    restored: prompts.map((prompt) => prompt.id),
  };
}

/** The stash items a message carries, which leave the stash once it is sent. */
export function stashIdsOf(attachments: readonly Attachment[], chips: readonly { id: string }[]): string[] {
  return [...attachments.flatMap((attachment) => (attachment.stashId ? [attachment.stashId] : [])), ...chips.map((chip) => chip.id)];
}
