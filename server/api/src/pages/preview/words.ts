export const MOST_TITLE = 200;
export const MOST_NAME = 80;

const CONTROLS = /[\p{Cc}\p{Zl}\p{Zp}]/gu;
/** Marks that turn the direction of the words around them, and code points no font draws. */
const UNSEEN =
  /[\u00ad\u061c\u200e\u200f\u202a-\u202e\u2060-\u206f\ufeff\p{Cn}\p{Co}\p{Cs}]/gu;

/** Words someone sent, on one line and at most `most` characters, or null when nothing is left. */
function plain(sent: string, most: number): string | null {
  const line = sent
    .normalize("NFC")
    .replace(CONTROLS, " ")
    .replace(UNSEEN, "")
    .replace(/\s+/gu, " ")
    .trim();
  const letters = Array.from(line);
  if (letters.length <= most) return line || null;
  return `${letters
    .slice(0, most - 1)
    .join("")
    .trimEnd()}…`;
}

export const plainTitle = (sent: string) => plain(sent, MOST_TITLE);

/** A sharer's name, unless it holds an email address. */
export const plainName = (sent: string) =>
  sent.includes("@") ? null : plain(sent, MOST_NAME);
