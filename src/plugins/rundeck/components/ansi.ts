export interface AnsiSegment {
    text: string;
    /** 0–15: the eight standard colours, then their bright forms. */
    fg: number | null;
    bg: number | null;
    bold: boolean;
}

interface Style {
    fg: number | null;
    bg: number | null;
    bold: boolean;
}

const PLAIN: Style = { fg: null, bg: null, bold: false };

const ESC = String.fromCharCode(27);
const BEL = String.fromCharCode(7);

// CSI sequences (ESC [ ... final byte), OSC sequences (ESC ] ... BEL or ESC \), and lone two-byte escapes.
const ESCAPE = new RegExp(`${ESC}\\[([0-9;:?]*)([@-~])|${ESC}\\][^${BEL}${ESC}]*(?:${BEL}|${ESC}\\\\)?|${ESC}[@-Z\\\\-_]?`, "g");

function applySgr(style: Style, params: string): Style {
    const codes = params === "" ? [0] : params.split(/[;:]/).map((part) => (part === "" ? 0 : Number(part)));
    let next = { ...style };
    for (let i = 0; i < codes.length; i += 1) {
        const code = codes[i];
        if (code === 0) next = { ...PLAIN };
        else if (code === 1) next.bold = true;
        else if (code === 22) next.bold = false;
        else if (code >= 30 && code <= 37) next.fg = code - 30;
        else if (code >= 90 && code <= 97) next.fg = code - 90 + 8;
        else if (code === 39) next.fg = null;
        else if (code >= 40 && code <= 47) next.bg = code - 40;
        else if (code >= 100 && code <= 107) next.bg = code - 100 + 8;
        else if (code === 49) next.bg = null;
        else if (code === 38 || code === 48) {
            const mode = codes[i + 1];
            i += mode === 5 ? 2 : mode === 2 ? 4 : 1;
        }
    }
    return next;
}

/** Splits a log line into styled runs. Colour escapes become styles; every other escape sequence is dropped. */
export function parseAnsi(input: string): AnsiSegment[] {
    if (!input.includes(ESC)) return input ? [{ text: input, ...PLAIN }] : [];
    const segments: AnsiSegment[] = [];
    let style: Style = { ...PLAIN };
    let last = 0;
    const push = (text: string) => {
        if (!text) return;
        const prev = segments[segments.length - 1];
        if (prev && prev.fg === style.fg && prev.bg === style.bg && prev.bold === style.bold) prev.text += text;
        else segments.push({ text, ...style });
    };
    for (const match of input.matchAll(ESCAPE)) {
        push(input.slice(last, match.index));
        last = match.index + match[0].length;
        if (match[2] === "m") style = applySgr(style, match[1] ?? "");
    }
    push(input.slice(last));
    return segments;
}

export function stripAnsi(input: string): string {
    return input.includes(ESC) ? input.replace(ESCAPE, "") : input;
}
