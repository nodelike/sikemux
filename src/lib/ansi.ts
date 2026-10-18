/** A palette index from 0 to 255, or a colour written out as `#rrggbb`. */
export type AnsiColor = number | string;

export interface AnsiRun {
    text: string;
    fg: AnsiColor | null;
    bg: AnsiColor | null;
    bold: boolean;
    dim: boolean;
    italic: boolean;
    underline: boolean;
}

type Style = Omit<AnsiRun, "text">;

const PLAIN: Style = { fg: null, bg: null, bold: false, dim: false, italic: false, underline: false };

const ESC = String.fromCharCode(27);
const BEL = String.fromCharCode(7);

// CSI sequences (ESC [ ... final byte), OSC sequences (ESC ] ... BEL or ESC \), and lone two-byte escapes.
const ESCAPE = new RegExp(`${ESC}\\[([0-9;:?]*)([@-~])|${ESC}\\][^${BEL}${ESC}]*(?:${BEL}|${ESC}\\\\)?|${ESC}[@-Z\\\\-_]?`, "g");

const hex = (value: number) => Math.max(0, Math.min(255, value)).toString(16).padStart(2, "0");

/** Reads `5;n` or `2;r;g;b` after a 38 or 48, returning the colour and how many codes it took. */
function extended(codes: readonly number[], at: number): [AnsiColor | null, number] {
    const mode = codes[at];
    if (mode === 5) return [codes[at + 1] ?? null, 2];
    if (mode === 2) return [`#${hex(codes[at + 1] ?? 0)}${hex(codes[at + 2] ?? 0)}${hex(codes[at + 3] ?? 0)}`, 4];
    return [null, 1];
}

function applySgr(style: Style, params: string): Style {
    const codes = params === "" ? [0] : params.split(/[;:]/).map((part) => (part === "" ? 0 : Number(part)));
    let next = { ...style };
    for (let at = 0; at < codes.length; at += 1) {
        const code = codes[at] ?? 0;
        if (code === 0) next = { ...PLAIN };
        else if (code === 1) next.bold = true;
        else if (code === 2) next.dim = true;
        else if (code === 3) next.italic = true;
        else if (code === 4) next.underline = true;
        else if (code === 22) next = { ...next, bold: false, dim: false };
        else if (code === 23) next.italic = false;
        else if (code === 24) next.underline = false;
        else if (code >= 30 && code <= 37) next.fg = code - 30;
        else if (code >= 90 && code <= 97) next.fg = code - 90 + 8;
        else if (code === 39) next.fg = null;
        else if (code >= 40 && code <= 47) next.bg = code - 40;
        else if (code >= 100 && code <= 107) next.bg = code - 100 + 8;
        else if (code === 49) next.bg = null;
        else if (code === 38 || code === 48) {
            const [color, used] = extended(codes, at + 1);
            if (code === 38) next.fg = color;
            else next.bg = color;
            at += used;
        }
    }
    return next;
}

const same = (a: Style, b: Style) =>
    a.fg === b.fg && a.bg === b.bg && a.bold === b.bold && a.dim === b.dim && a.italic === b.italic && a.underline === b.underline;

/** Splits a line into styled runs. Colour and weight escapes become styles; every other escape sequence is dropped. */
export function parseAnsi(input: string): AnsiRun[] {
    if (!input.includes(ESC)) return input ? [{ text: input, ...PLAIN }] : [];
    const runs: AnsiRun[] = [];
    let style: Style = { ...PLAIN };
    let last = 0;
    const push = (text: string) => {
        if (!text) return;
        const prev = runs[runs.length - 1];
        if (prev && same(prev, style)) prev.text += text;
        else runs.push({ text, ...style });
    };
    for (const match of input.matchAll(ESCAPE)) {
        push(input.slice(last, match.index));
        last = match.index + match[0].length;
        if (match[2] === "m") style = applySgr(style, match[1] ?? "");
    }
    push(input.slice(last));
    return runs;
}

export function stripAnsi(input: string): string {
    return input.includes(ESC) ? input.replace(ESCAPE, "") : input;
}

const CUBE = [0, 95, 135, 175, 215, 255];

/** A colour as CSS. The sixteen named colours are the terminal's own, so a log reads like the terminal does. */
export function ansiCss(color: AnsiColor): string {
    if (typeof color === "string") return color;
    if (color < 16) return `var(--terminal-ansi-${color})`;
    if (color < 232) {
        const index = color - 16;
        return `#${hex(CUBE[Math.floor(index / 36)] ?? 0)}${hex(CUBE[Math.floor(index / 6) % 6] ?? 0)}${hex(CUBE[index % 6] ?? 0)}`;
    }
    const gray = 8 + (color - 232) * 10;
    return `#${hex(gray)}${hex(gray)}${hex(gray)}`;
}
