/*
 * A picture the message carries is named in its words as `[Image #n]`, attachment n, where the host puts it,
 * as the Mac's draft names it.
 */
import { editDraft, type DraftToken } from './mentions';
import type { Attachment } from './uploads';

export const IMAGE = 'image:';
const PATTERN = /\[Image #(\d+)\]/g;

const tag = (number: number) => `[Image #${number}]`;

/** `text` with `tags` written at `caret`, spaced from the words around them, and where each tag landed. */
function written(text: string, caret: number | undefined, tags: string[]) {
  const at = Math.min(caret ?? text.length, text.length);
  const before = text.slice(0, at);
  const after = text.slice(at);
  const lead = before && !/\s$/.test(before) ? ' ' : '';
  const trail = /^\s/.test(after) ? '' : ' ';
  let start = at + lead.length;
  const places = tags.map((written, index) => {
    if (index > 0) start += 1;
    const place = { start, end: start + written.length };
    start = place.end;
    return place;
  });
  return { text: `${before}${lead}${tags.join(' ')}${trail}${after}`, places };
}

/** A draft with chips: the tags of the pictures among `added`, numbered after `held`, written at `caret` as chips. */
export function tagPictures(
  text: string,
  tokens: readonly DraftToken[],
  held: number,
  added: readonly Attachment[],
  caret: number | undefined,
): { text: string; tokens: DraftToken[] } {
  const pictures = added.flatMap((file, index) => (file.kind === 'image' ? [{ file, number: held + index + 1 }] : []));
  if (!pictures.length) return { text, tokens: [...tokens] };
  const next = written(
    text,
    caret,
    pictures.map(({ number }) => tag(number)),
  );
  const moved = editDraft(text, next.text, tokens).tokens;
  const placed = pictures.map(({ file, number }, index) => ({
    ...next.places[index],
    id: `${IMAGE}${file.id}`,
    label: `Image ${number}`,
  }));
  return { text: next.text, tokens: [...moved, ...placed].sort((a, b) => a.start - b.start) };
}

/** A draft with chips, once the file `gone` is off `left`: its tags go and the later pictures are counted down. */
export function untagPicture(
  text: string,
  tokens: readonly DraftToken[],
  gone: string,
  left: readonly Attachment[],
): { text: string; tokens: DraftToken[] } {
  let next = text;
  let shift = 0;
  const kept: DraftToken[] = [];
  for (const token of [...tokens].sort((a, b) => a.start - b.start)) {
    const start = token.start + shift;
    const end = token.end + shift;
    if (token.id === `${IMAGE}${gone}`) {
      next = next.slice(0, start) + next.slice(end);
      shift -= end - start;
      continue;
    }
    const index = token.id.startsWith(IMAGE) ? left.findIndex((file) => `${IMAGE}${file.id}` === token.id) : -1;
    if (index >= 0) {
      const words = tag(index + 1);
      next = next.slice(0, start) + words + next.slice(end);
      shift += words.length - (end - start);
      kept.push({ ...token, start, end: start + words.length, label: `Image ${index + 1}` });
      continue;
    }
    kept.push({ ...token, start, end });
  }
  return { text: next, tokens: kept };
}

/** For a draft that keeps its tags only in its words, as a new chat's does: `text` with each of `numbers` tagged at `caret`. */
export function writeTags(text: string, caret: number | undefined, numbers: number[]): string {
  return numbers.length ? written(text, caret, numbers.map(tag)).text : text;
}

/** `text` without attachment `removed`'s tag, the later ones counted down so they still name theirs. */
export function dropTag(text: string, removed: number): string {
  return text.replace(PATTERN, (whole, digits: string) => {
    const number = Number(digits);
    return number === removed ? '' : number > removed ? tag(number - 1) : whole;
  });
}

/** The tags in `text` that name one of its `count` attachments, as chips to draw. */
export function tagTokens(text: string, count: number): DraftToken[] {
  return [...text.matchAll(PATTERN)].flatMap((match) => {
    const number = Number(match[1]);
    if (number < 1 || number > count || match.index === undefined) return [];
    return [{ start: match.index, end: match.index + match[0].length, id: `${IMAGE}${number}`, label: `Image ${number}` }];
  });
}
