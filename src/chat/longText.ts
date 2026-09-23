/* A message this long is thousands of words. Laying one out costs the engine a
   full pass of glyph shaping every time something asks the document for its
   layout — which macOS does on every hover — so the tail is held back until
   the reader asks for it. */
export const LONG_TEXT_LIMIT = 20_000;

export interface TextCut {
    /** The part that renders, already safe to hand to markdown. */
    head: string;
    /** How much is being held back. */
    hidden: number;
}

/**
 * Where a long message can be folded. The cut prefers a paragraph break, then
 * any line break, and only falls back to the exact limit when the text has no
 * break to use — a single enormous line is precisely the case this is for.
 */
export function cutLongText(text: string, limit = LONG_TEXT_LIMIT): TextCut | null {
    if (text.length <= limit) return null;

    const floor = Math.floor(limit / 2);
    let at = text.lastIndexOf("\n\n", limit);
    if (at < floor) at = text.lastIndexOf("\n", limit);
    if (at < floor) at = limit;

    let head = text.slice(0, at);
    // A fence opened before the cut has to be closed here, or every paragraph
    // after it renders as code.
    if ((head.match(/^```/gm) ?? []).length % 2 === 1) head += "\n```";
    return { head, hidden: text.length - at };
}
