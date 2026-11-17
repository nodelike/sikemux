import { AGENT_URI, EARLIER_URI } from '@shared/wire.generated';
import type { ContextChip } from '@/stash/words';

/**
 * A chip the draft holds, over the text it wrote, as every app keeps it: `agent:<id>`, `file:<path>`,
 * `folder:<path>`, `earlier:<cli>/<session>` or `tracked:<kind>:<number>:<url>`.
 */
export type DraftToken = { start: number; end: number; id: string; label?: string };

export type Trigger = '@' | '+' | '#' | '!';

/** A word that asks for something to name, and where it starts. */
export type Typed = { trigger: Trigger; start: number; needle: string };

const TYPED = /(^|\s)([@+#!])(\S*)$/;

/** The word at the end of `draft` that asks for something to name, unless it is a chip already. */
export function typedMention(draft: string, tokens: readonly DraftToken[]): Typed | undefined {
  const found = TYPED.exec(draft);
  if (!found) return undefined;
  const start = found.index + found[1].length;
  if (tokens.some((token) => token.start === start)) return undefined;
  return { trigger: found[2] as Trigger, start, needle: found[3] };
}

export const AGENT = 'agent:';
export const FILE = 'file:';
export const FOLDER = 'folder:';
export const EARLIER = 'earlier:';
export const TRACKED = 'tracked:';

export const trackedId = (kind: string, number: number, url: string) => `${TRACKED}${kind}:${number}:${url}`;

/**
 * `after` as an edit of `before` leaves the chips: those before or after the change move with it and those
 * it cuts into go. Taking text out of a chip takes the whole chip out, as the Mac's draft does.
 */
export function editDraft(before: string, after: string, tokens: readonly DraftToken[]): { text: string; tokens: DraftToken[] } {
  let start = 0;
  while (start < before.length && start < after.length && before[start] === after[start]) start += 1;
  let tail = 0;
  while (
    tail < before.length - start &&
    tail < after.length - start &&
    before[before.length - 1 - tail] === after[after.length - 1 - tail]
  ) {
    tail += 1;
  }
  let removedEnd = before.length - tail;
  let text = after;
  const cut = tokens.filter((token) => token.end > start && token.start < removedEnd);
  if (cut.length && after.length - tail === start) {
    start = Math.min(start, ...cut.map((token) => token.start));
    removedEnd = Math.max(removedEnd, ...cut.map((token) => token.end));
    text = before.slice(0, start) + before.slice(removedEnd);
  }
  const moved = text.length - before.length;
  return {
    text,
    tokens: tokens.flatMap((token) => {
      if (token.end <= start) return [token];
      if (token.start >= removedEnd) return [{ ...token, start: token.start + moved, end: token.end + moved }];
      return [];
    }),
  };
}

function utf8Length(text: string): number {
  let bytes = 0;
  for (const letter of text) {
    const code = letter.codePointAt(0) ?? 0;
    bytes += code < 0x80 ? 1 : code < 0x800 ? 2 : code < 0x10000 ? 3 : 4;
  }
  return bytes;
}

function indexOfByte(text: string, byte: number): number | undefined {
  let bytes = 0;
  let index = 0;
  for (const letter of text) {
    if (bytes === byte) return index;
    bytes += utf8Length(letter);
    index += letter.length;
  }
  return bytes === byte ? index : undefined;
}

/** The chips a draft from the host holds, whose places it counts in UTF-8 bytes; one that no longer fits its text is left out. */
export function tokensOf(text: string, kept: unknown): DraftToken[] {
  if (!Array.isArray(kept)) return [];
  return kept.flatMap((token: { start?: unknown; end?: unknown; id?: unknown; label?: unknown }) => {
    if (typeof token?.start !== 'number' || typeof token.end !== 'number' || typeof token.id !== 'string') return [];
    const start = indexOfByte(text, token.start);
    const end = indexOfByte(text, token.end);
    if (start === undefined || end === undefined || end <= start) return [];
    return [{ start, end, id: token.id, ...(typeof token.label === 'string' ? { label: token.label } : {}) }];
  });
}

export function keptTokens(text: string, tokens: readonly DraftToken[]) {
  return tokens.map((token) => ({
    start: utf8Length(text.slice(0, token.start)),
    end: utf8Length(text.slice(0, token.end)),
    id: token.id,
    ...(token.label ? { label: token.label } : {}),
  }));
}

/** The issues and pull requests a draft from the host carries, by the chip they go with. */
export function trackedOf(kept: unknown): Record<string, ContextChip> {
  if (!kept || typeof kept !== 'object') return {};
  return Object.fromEntries(
    Object.entries(kept as Record<string, ContextChip>).filter(
      ([, item]) => typeof item?.uri === 'string' && typeof item.title === 'string' && typeof item.text === 'string',
    ),
  );
}

/** What the chips in `text` send besides its words: the files they name, and the agents, chats and issues for the host to read. */
export function outgoing(text: string, tokens: readonly DraftToken[], tracked: Readonly<Record<string, ContextChip>>) {
  const paths: string[] = [];
  const context: ContextChip[] = [];
  const add = (item: ContextChip) => {
    if (!context.some((held) => held.uri === item.uri)) context.push(item);
  };
  for (const token of tokens) {
    const written = text.slice(token.start, token.end);
    const path = token.id.startsWith(FILE)
      ? token.id.slice(FILE.length)
      : token.id.startsWith(FOLDER)
        ? token.id.slice(FOLDER.length)
        : null;
    if (tracked[token.id]) add(tracked[token.id]);
    else if (path !== null) {
      if (!paths.includes(path)) paths.push(path);
    } else if (token.id.startsWith(AGENT)) add({ uri: `${AGENT_URI}${token.id.slice(AGENT.length)}`, title: written, text: '' });
    else if (token.id.startsWith(EARLIER)) add({ uri: `${EARLIER_URI}${token.id.slice(EARLIER.length)}`, title: written, text: '' });
  }
  return { paths, context };
}
