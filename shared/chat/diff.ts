import type { DiffLine } from "./types";

function splitLines(text: string): string[] {
    if (text === "") return [];
    const lines = text.split("\n");
    if (lines.length > 1 && lines.at(-1) === "") lines.pop();
    return lines;
}

/* Where a single deleted line and a single added line differ, so the row can
   show the changed words rather than making the reader compare two lines. */
function markPair(deleted: DiffLine, added: DiffLine): void {
    const a = deleted.text;
    const b = added.text;
    if (a.length > 200 || b.length > 200) return;
    let start = 0;
    while (start < a.length && start < b.length && a[start] === b[start]) start += 1;
    let end = 0;
    while (end < a.length - start && end < b.length - start && a[a.length - 1 - end] === b[b.length - 1 - end]) end += 1;
    /* Two lines that merely end in the same bracket are not an edit of one
       another; a span is only worth marking when most of the line survived. */
    const shared = start + end;
    if (shared < 3 || shared < Math.min(a.length, b.length) * 0.25) return;
    deleted.mark = [start, a.length - end];
    added.mark = [start, b.length - end];
}

function markPairs(lines: DiffLine[]): void {
    for (let index = 0; index < lines.length - 1; index += 1) {
        const deleted = lines[index];
        const added = lines[index + 1];
        if (deleted.sign !== "-" || added.sign !== "+") continue;
        if (lines[index - 1]?.sign === "-" || lines[index + 2]?.sign === "+") continue;
        markPair(deleted, added);
    }
}

/* A fenced block the agent wrote is sometimes a patch. It has no line numbers
   to speak of, but the signs alone are worth colouring, and a reader should not
   have to compare two near-identical lines by eye. */
export function fencedDiff(text: string, language?: string): DiffLine[] | null {
    const raw = splitLines(text);
    if (raw.length < 2 || raw.length > 400) return null;

    const signed = raw.filter((line) => line.startsWith("+") || line.startsWith("-"));
    const named = language === "diff" || language === "patch";
    if (!named) {
        const shaped = raw.filter((line) => /^[-+@ ]/.test(line) || line === "");
        const adds = raw.some((line) => line.startsWith("+"));
        const dels = raw.some((line) => line.startsWith("-"));
        if (!adds || !dels || signed.length < 2 || shaped.length < raw.length * 0.8) return null;
    }
    if (signed.length === 0) return null;

    const lines: DiffLine[] = raw.map((line) => ({
        sign: line.startsWith("+") ? "+" : line.startsWith("-") ? "-" : " ",
        text: /^[-+ ]/.test(line) ? line.slice(1) : line,
    }));
    markPairs(lines);
    return lines;
}

export interface DiffView {
    rows: (DiffLine | { gap: number })[];
    /** Unchanged lines folded away, which the reader can ask for. */
    hidden: number;
}

/* Only the lines around a change are worth the room; the rest fold into a gap
   the reader can open. */
export function collapseDiff(lines: DiffLine[], context: number): DiffView {
    const keep = new Array<boolean>(lines.length).fill(false);
    for (let index = 0; index < lines.length; index += 1) {
        if (lines[index].sign === " ") continue;
        for (let near = Math.max(0, index - context); near <= Math.min(lines.length - 1, index + context); near += 1) keep[near] = true;
    }

    const rows: (DiffLine | { gap: number })[] = [];
    let hidden = 0;
    let run = 0;
    for (let index = 0; index < lines.length; index += 1) {
        if (keep[index]) {
            if (run > 0) {
                rows.push({ gap: run });
                hidden += run;
                run = 0;
            }
            rows.push(lines[index]);
        } else {
            run += 1;
        }
    }
    if (run > 0) {
        rows.push({ gap: run });
        hidden += run;
    }
    return { rows, hidden };
}
