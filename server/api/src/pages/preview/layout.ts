import type { Font } from "./font.ts";

export const TITLE_SPACING = -0.018;
export const LINE_HEIGHT = 1.1;

const cutToFit = (line: string, fits: (line: string) => boolean): string => {
  const letters = Array.from(line);
  while (letters.length && !fits(`${letters.join("").trimEnd()}…`))
    letters.pop();
  return `${letters.join("").trimEnd()}…`;
};

export function shorten(
  font: Font,
  text: string,
  size: number,
  width: number,
): string {
  const fits = (line: string) => font.width(line, size) <= width;
  return fits(text) ? text : cutToFit(text, fits);
}

/** The lines `words` wrap to, or null when one word is wider than a line. */
function wrap(
  words: string[],
  fits: (line: string) => boolean,
): string[] | null {
  const lines: string[] = [];
  let line = "";
  for (const word of words) {
    const longer = line ? `${line} ${word}` : word;
    if (fits(longer)) {
      line = longer;
      continue;
    }
    if (!line || !fits(word)) return null;
    lines.push(line);
    line = word;
  }
  return [...lines, line];
}

/** Breaks each word too wide for a line into pieces that fit one. */
function pieces(words: string[], fits: (line: string) => boolean): string[] {
  return words.flatMap((word) => {
    const out: string[] = [];
    let piece = "";
    for (const letter of word) {
      if (piece && !fits(piece + letter)) {
        out.push(piece);
        piece = "";
      }
      piece += letter;
    }
    return [...out, piece];
  });
}

/**
 * The title's lines at the largest size that fits, each about as long as the next. At the smallest
 * size a word too long for a line is broken and a title too long for the card is cut.
 */
export function layTitle(
  font: Font,
  title: string,
  { width, height, sizes }: { width: number; height: number; sizes: number[] },
): { size: number; lines: string[] } {
  const words = title.split(" ");
  const within = (size: number, width: number) => (line: string) =>
    font.width(line, size, TITLE_SPACING) <= width;
  const most = (size: number) => Math.floor(height / (size * LINE_HEIGHT));
  for (const size of sizes) {
    const widest = wrap(words, within(size, width));
    if (!widest || widest.length > most(size)) continue;
    let lines = widest;
    for (let narrower = width - 12; narrower > 120; narrower -= 12) {
      const next = wrap(words, within(size, narrower));
      if (!next || next.length > widest.length) break;
      lines = next;
    }
    return { size, lines };
  }
  const size = sizes.at(-1) ?? 46;
  const fits = within(size, width);
  const all = wrap(pieces(words, fits), fits) ?? [];
  const lines = all.slice(0, most(size));
  const last = lines.at(-1);
  if (all.length > lines.length && last !== undefined)
    lines[lines.length - 1] = cutToFit(last, fits);
  return { size, lines };
}
