import { recordOf } from "../lib/guards";
import type { AcpToolCall } from "./types";

export interface DiffLine {
    sign: " " | "+" | "-";
    text: string;
    oldLine?: number;
    newLine?: number;
    /** The span that actually changed, when one deleted line pairs with one added line. */
    mark?: [number, number];
}

export interface ToolDiff {
    path: string;
    adds: number;
    dels: number;
    lines: DiffLine[];
}

/* Past this the transcript is the wrong place to read the change — the row
   still names the file, and the diff pane shows the whole thing. */
const MAX_LINES = 1200;

function splitLines(text: string): string[] {
    if (text === "") return [];
    const lines = text.split("\n");
    if (lines.length > 1 && lines.at(-1) === "") lines.pop();
    return lines;
}

function textOf(value: unknown): string | undefined {
    return typeof value === "string" ? value : undefined;
}

/* The longest run of lines the two sides share, as a grid of match lengths.
   Only ever asked about the middle, which is what keeps the grid small. */
function commonRuns(before: string[], after: string[]): number[][] {
    const grid: number[][] = Array.from({ length: before.length + 1 }, () => new Array<number>(after.length + 1).fill(0));
    for (let i = before.length - 1; i >= 0; i -= 1) {
        for (let j = after.length - 1; j >= 0; j -= 1) {
            grid[i][j] = before[i] === after[j] ? grid[i + 1][j + 1] + 1 : Math.max(grid[i + 1][j], grid[i][j + 1]);
        }
    }
    return grid;
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

export function diffLines(oldText: string, newText: string): DiffLine[] | null {
    const before = splitLines(oldText);
    const after = splitLines(newText);
    if (before.length + after.length > MAX_LINES) return null;

    const lines: DiffLine[] = [];
    let oldLine = 1;
    let newLine = 1;

    let head = 0;
    while (head < before.length && head < after.length && before[head] === after[head]) head += 1;
    let tail = 0;
    while (tail < before.length - head && tail < after.length - head && before[before.length - 1 - tail] === after[after.length - 1 - tail]) {
        tail += 1;
    }

    for (let index = 0; index < head; index += 1) {
        lines.push({ sign: " ", text: before[index], oldLine: oldLine++, newLine: newLine++ });
    }

    const middleBefore = before.slice(head, before.length - tail);
    const middleAfter = after.slice(head, after.length - tail);
    const grid = commonRuns(middleBefore, middleAfter);
    let i = 0;
    let j = 0;
    while (i < middleBefore.length && j < middleAfter.length) {
        if (middleBefore[i] === middleAfter[j]) {
            lines.push({ sign: " ", text: middleBefore[i], oldLine: oldLine++, newLine: newLine++ });
            i += 1;
            j += 1;
        } else if (grid[i + 1][j] >= grid[i][j + 1]) {
            lines.push({ sign: "-", text: middleBefore[i], oldLine: oldLine++ });
            i += 1;
        } else {
            lines.push({ sign: "+", text: middleAfter[j], newLine: newLine++ });
            j += 1;
        }
    }
    while (i < middleBefore.length) lines.push({ sign: "-", text: middleBefore[i++], oldLine: oldLine++ });
    while (j < middleAfter.length) lines.push({ sign: "+", text: middleAfter[j++], newLine: newLine++ });

    for (let index = before.length - tail; index < before.length; index += 1) {
        lines.push({ sign: " ", text: before[index], oldLine: oldLine++, newLine: newLine++ });
    }

    markPairs(lines);
    return lines;
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

/* An adapter that follows the ACP spec hands us the change itself. */
function fromContent(tool: AcpToolCall): { path: string; oldText: string; newText: string } | null {
    if (!Array.isArray(tool.content)) return null;
    for (const item of tool.content) {
        const block = recordOf(item);
        const inner = recordOf(block?.content) ?? block;
        if (!inner || inner.type !== "diff") continue;
        const path = textOf(inner.path);
        const newText = textOf(inner.newText);
        if (path === undefined || newText === undefined) continue;
        return { path, oldText: textOf(inner.oldText) ?? "", newText };
    }
    return null;
}

/* One that does not still tells us what it was asked to write. */
function fromRawInput(tool: AcpToolCall): { path: string; oldText: string; newText: string } | null {
    const input = recordOf(tool.rawInput);
    if (!input) return null;
    const path = textOf(input.file_path) ?? textOf(input.path) ?? textOf(input.filePath);
    if (path === undefined) return null;
    const oldText = textOf(input.old_string) ?? textOf(input.oldText);
    const newText = textOf(input.new_string) ?? textOf(input.newText) ?? textOf(input.content);
    if (newText === undefined) return null;
    return { path, oldText: oldText ?? "", newText };
}

/* What a failed call left behind, short enough to sit under it. Anything
   longer belongs in the terminal the call came from. */
const MAX_FAILURE_CHARS = 400;

export function toolFailure(tool: AcpToolCall): string | null {
    if (tool.status !== "failed") return null;
    const output = tool.rawOutput;
    const record = recordOf(output);
    const raw = typeof output === "string" ? output : (textOf(record?.output) ?? textOf(record?.stderr) ?? textOf(record?.error) ?? null);
    const trimmed = raw?.trim();
    if (!trimmed) return null;
    return trimmed.length > MAX_FAILURE_CHARS ? `${trimmed.slice(0, MAX_FAILURE_CHARS)}…` : trimmed;
}

export function toolDiff(tool: AcpToolCall): ToolDiff | null {
    const source = fromContent(tool) ?? fromRawInput(tool);
    if (!source) return null;
    const lines = diffLines(source.oldText, source.newText);
    if (!lines) return null;
    let adds = 0;
    let dels = 0;
    for (const line of lines) {
        if (line.sign === "+") adds += 1;
        else if (line.sign === "-") dels += 1;
    }
    if (adds === 0 && dels === 0) return null;
    return { path: source.path, adds, dels, lines };
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
