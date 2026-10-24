/*
 * A terminal's cells as the core sends them; the layout is sikemux-wire's
 * grid.rs. The core runs the terminal and sends each pane only what changed
 * since its last frame, and lines that scrolled off the top in pages when
 * asked. Lines are numbered from the first one the terminal ever pushed into
 * its history, so a page fetched earlier stays valid until `epoch` changes.
 */

export const FRAME_RESET = 1;
const FRAME_PALETTE = 2;

export const MODE_ALT_SCREEN = 1;
export const MODE_MOUSE_CLICK = 1 << 1;
export const MODE_MOUSE_DRAG = 1 << 2;
export const MODE_MOUSE_MOTION = 1 << 3;
export const MODE_ALTERNATE_SCROLL = 1 << 4;
export const MODE_MOUSE_ANY = MODE_MOUSE_CLICK | MODE_MOUSE_DRAG | MODE_MOUSE_MOTION;

export const STYLE_BOLD = 1;
export const STYLE_ITALIC = 1 << 1;
export const STYLE_DIM = 1 << 2;
export const STYLE_INVERSE = 1 << 3;
export const STYLE_HIDDEN = 1 << 4;
export const STYLE_STRIKE = 1 << 5;
export const STYLE_UNDERLINE = 1 << 6;
export const STYLE_DOUBLE_UNDERLINE = 1 << 7;
export const STYLE_CURLY_UNDERLINE = 1 << 8;
export const STYLE_DOTTED_UNDERLINE = 1 << 9;
export const STYLE_DASHED_UNDERLINE = 1 << 10;
export const STYLE_ANY_UNDERLINE = STYLE_UNDERLINE | STYLE_DOUBLE_UNDERLINE | STYLE_CURLY_UNDERLINE | STYLE_DOTTED_UNDERLINE | STYLE_DASHED_UNDERLINE;
const STYLE_UNDERLINE_COLOR = 1 << 14;
const STYLE_LINK = 1 << 15;

export const CURSOR_HIDDEN = 0;
export const CURSOR_BLOCK = 1;
export const CURSOR_UNDERLINE = 2;
export const CURSOR_BEAM = 3;
export const CURSOR_HOLLOW = 4;
export const CURSOR_BLINKS = 0x80;

const SPAN_WIDE = 1;
const SPAN_CLUSTER = 2;
const ROW_WRAPPED = 1;

const OP_END = 0;
const OP_COPY = 1;
const OP_CLEAR = 2;
const OP_ROW = 3;

/** Theme colour numbers: 0-255 the palette, 256 foreground, 257 background, 258 cursor, 259-266 dim, 267-268 bright and dim foreground. */
export const COLOR_FOREGROUND = 256;
export const COLOR_BACKGROUND = 257;
/** A colour the program gave as RGB, kept as `COLOR_RGB | 0xrrggbb`. */
export const COLOR_RGB = 0x1000000;

export interface Style {
    readonly fg: number;
    readonly bg: number;
    readonly flags: number;
    readonly underline: number | null;
    readonly link: string | null;
}

export interface Span {
    readonly style: Style;
    readonly text: string;
    /** The column the span starts in. */
    readonly col: number;
    /** Columns the span covers. */
    readonly width: number;
    /** Each character takes two columns. */
    readonly wide: boolean;
    /** The whole text is one cell: a character with marks combined onto it. */
    readonly cluster: boolean;
}

export interface Row {
    readonly wrapped: boolean;
    readonly spans: readonly Span[];
}

export const BLANK_ROW: Row = Object.freeze({ wrapped: false, spans: Object.freeze([]) });

export interface Cursor {
    readonly row: number;
    readonly col: number;
    readonly shape: number;
}

export interface FrameHeader {
    readonly frame: number;
    readonly flags: number;
    readonly cols: number;
    readonly rows: number;
    readonly epoch: number;
    readonly historyEnd: number;
    readonly historyLen: number;
    readonly cursor: Cursor;
    readonly modes: number;
    readonly palette: ReadonlyMap<number, number> | null;
}

export type Op =
    | { readonly kind: "copy"; readonly dst: number; readonly src: number; readonly count: number }
    | { readonly kind: "clear"; readonly row: number; readonly count: number }
    | { readonly kind: "row"; readonly index: number; readonly row: Row };

export interface Frame {
    readonly header: FrameHeader;
    readonly ops: readonly Op[];
}

export interface Lines {
    readonly epoch: number;
    readonly start: number;
    readonly rows: readonly Row[];
}

const utf8 = new TextDecoder();

class Reader {
    offset = 0;
    private readonly bytes: Uint8Array;

    constructor(bytes: Uint8Array) {
        this.bytes = bytes;
    }

    byte(): number {
        if (this.offset >= this.bytes.length) throw new RangeError("terminal frame ended early");
        return this.bytes[this.offset++];
    }

    varint(): number {
        let value = 0;
        let scale = 1;
        for (;;) {
            const byte = this.byte();
            value += (byte & 0x7f) * scale;
            if (byte < 0x80) return value;
            scale *= 128;
        }
    }

    take(length: number): Uint8Array {
        if (this.offset + length > this.bytes.length) throw new RangeError("terminal frame ended early");
        const slice = this.bytes.subarray(this.offset, this.offset + length);
        this.offset += length;
        return slice;
    }

    color(): number {
        const tag = this.byte();
        if (tag === 0) return this.varint();
        if (tag === 1) return this.byte();
        const [r, g, b] = this.take(3);
        return COLOR_RGB | (r << 16) | (g << 8) | b;
    }

    styles(): Style[] {
        const count = this.varint();
        const raw: { flags: number; fg: number; bg: number; underline: number | null; link: number | null }[] = [];
        for (let index = 0; index < count; index++) {
            const flags = this.varint();
            const fg = this.color();
            const bg = this.color();
            const underline = flags & STYLE_UNDERLINE_COLOR ? this.color() : null;
            const link = flags & STYLE_LINK ? this.varint() : null;
            raw.push({ flags: flags & ~(STYLE_UNDERLINE_COLOR | STYLE_LINK), fg, bg, underline, link });
        }
        const links: string[] = [];
        const linkCount = this.varint();
        for (let index = 0; index < linkCount; index++) links.push(utf8.decode(this.take(this.varint())));
        return raw.map((style) => ({ ...style, link: style.link === null ? null : (links[style.link] ?? null) }));
    }

    row(styles: readonly Style[]): Row {
        const wrapped = (this.byte() & ROW_WRAPPED) !== 0;
        const spans: Span[] = [];
        let col = 0;
        for (;;) {
            const reference = this.varint();
            if (reference === 0) return { wrapped, spans };
            const style = styles[reference - 1];
            if (!style) throw new RangeError("terminal frame names a style it does not carry");
            const flags = this.byte();
            const text = utf8.decode(this.take(this.varint()));
            const wide = (flags & SPAN_WIDE) !== 0;
            const cluster = (flags & SPAN_CLUSTER) !== 0;
            let cells = 1;
            if (!cluster) {
                cells = 0;
                for (const _ of text) cells++;
            }
            const width = wide ? cells * 2 : cells;
            spans.push({ style, text, col, width, wide, cluster });
            col += width;
        }
    }
}

export function decodeFrame(bytes: Uint8Array): Frame {
    const reader = new Reader(bytes);
    const frame = reader.varint();
    const flags = reader.byte();
    const cols = reader.varint();
    const rows = reader.varint();
    const epoch = reader.varint();
    const historyEnd = reader.varint();
    const historyLen = reader.varint();
    const cursor = { row: reader.varint(), col: reader.varint(), shape: reader.byte() };
    const modes = reader.varint();
    let palette: Map<number, number> | null = null;
    if (flags & FRAME_PALETTE) {
        palette = new Map();
        const count = reader.varint();
        for (let index = 0; index < count; index++) {
            const at = reader.varint();
            const [r, g, b] = reader.take(3);
            palette.set(at, (r << 16) | (g << 8) | b);
        }
    }
    const styles = reader.styles();
    const ops: Op[] = [];
    for (;;) {
        const op = reader.byte();
        if (op === OP_END) break;
        if (op === OP_COPY) ops.push({ kind: "copy", dst: reader.varint(), src: reader.varint(), count: reader.varint() });
        else if (op === OP_CLEAR) ops.push({ kind: "clear", row: reader.varint(), count: reader.varint() });
        else if (op === OP_ROW) {
            const index = reader.varint();
            ops.push({ kind: "row", index, row: reader.row(styles) });
        } else throw new RangeError(`unknown terminal frame operation ${op}`);
    }
    return { header: { frame, flags, cols, rows, epoch, historyEnd, historyLen, cursor, modes, palette }, ops };
}

export function decodeLines(bytes: Uint8Array): Lines {
    const reader = new Reader(bytes);
    const epoch = reader.varint();
    const start = reader.varint();
    const count = reader.varint();
    const styles = reader.styles();
    const rows: Row[] = [];
    for (let index = 0; index < count; index++) rows.push(reader.row(styles));
    return { epoch, start, rows };
}

/** The text of a row, one entry per column; the second column of a wide character is empty. */
export function rowCells(row: Row, cols: number): string[] {
    const cells: string[] = new Array<string>(cols).fill(" ");
    for (const span of row.spans) {
        if (span.cluster) {
            if (span.col < cols) cells[span.col] = span.text;
            if (span.wide && span.col + 1 < cols) cells[span.col + 1] = "";
            continue;
        }
        let col = span.col;
        for (const character of span.text) {
            if (col >= cols) break;
            cells[col] = character;
            if (span.wide && col + 1 < cols) cells[col + 1] = "";
            col += span.wide ? 2 : 1;
        }
    }
    return cells;
}

export function rowText(row: Row, cols: number): string {
    return rowCells(row, cols).join("").trimEnd();
}

/** The span covering `col`, if any. */
export function spanAt(row: Row, col: number): Span | undefined {
    return row.spans.find((span) => col >= span.col && col < span.col + span.width);
}

const HISTORY_CACHE_LINES = 4_000;

/**
 * A pane's copy of a terminal: the screen, kept current by frames, and the
 * history lines it fetched, dropped once their numbering is out of date.
 */
export class TerminalGrid {
    header: FrameHeader = {
        frame: 0,
        flags: 0,
        cols: 0,
        rows: 0,
        epoch: -1,
        historyEnd: 0,
        historyLen: 0,
        cursor: { row: 0, col: 0, shape: CURSOR_HIDDEN },
        modes: 0,
        palette: null,
    };
    palette: ReadonlyMap<number, number> = new Map();
    rows: Row[] = [];
    private history = new Map<number, Row>();

    get cols(): number {
        return this.header.cols;
    }

    get historyStart(): number {
        return this.header.historyEnd - this.header.historyLen;
    }

    /** Applies a frame, answering which screen rows it changed. */
    apply(frame: Frame): Set<number> {
        const { header, ops } = frame;
        const reset = (header.flags & FRAME_RESET) !== 0;
        const previous = reset ? [] : this.rows;
        const next = previous.slice(0, header.rows);
        while (next.length < header.rows) next.push(BLANK_ROW);
        const changed = new Set<number>();
        if (reset) for (let index = 0; index < header.rows; index++) next[index] = BLANK_ROW;
        for (const op of ops) {
            if (op.kind === "copy") {
                for (let offset = 0; offset < op.count; offset++) {
                    const dst = op.dst + offset;
                    if (dst < next.length) next[dst] = previous[op.src + offset] ?? BLANK_ROW;
                    changed.add(dst);
                }
            } else if (op.kind === "clear") {
                for (let offset = 0; offset < op.count; offset++) {
                    if (op.row + offset < next.length) next[op.row + offset] = BLANK_ROW;
                    changed.add(op.row + offset);
                }
            } else if (op.index < next.length) {
                next[op.index] = op.row;
                changed.add(op.index);
            }
        }
        if (reset || header.epoch !== this.header.epoch || header.cols !== this.header.cols) this.history.clear();
        if (header.palette) this.palette = header.palette;
        this.header = header;
        this.rows = next;
        if (reset) for (let index = 0; index < header.rows; index++) changed.add(index);
        return changed;
    }

    applyLines(lines: Lines): void {
        if (lines.epoch !== this.header.epoch) return;
        lines.rows.forEach((row, offset) => this.history.set(lines.start + offset, row));
        if (this.history.size > HISTORY_CACHE_LINES) {
            const keep = this.header.historyEnd - HISTORY_CACHE_LINES;
            for (const line of this.history.keys()) if (line < keep) this.history.delete(line);
        }
    }

    /** The row with line number `line`, or nothing while that history line is not fetched. */
    line(line: number): Row | undefined {
        if (line >= this.header.historyEnd) return this.rows[line - this.header.historyEnd];
        if (line < this.historyStart) return undefined;
        return this.history.get(line);
    }

    hasLine(line: number): boolean {
        return line >= this.header.historyEnd || line < this.historyStart || this.history.has(line);
    }
}

export interface Mods {
    shift?: boolean;
    alt?: boolean;
    ctrl?: boolean;
    meta?: boolean;
}

export type MouseButton = "left" | "middle" | "right" | "none";

/** What a person does to a terminal; the core encodes it for the program. */
export type TerminalInput =
    | { type: "key"; key: string; mods: Mods }
    | { type: "text"; text: string }
    | { type: "paste"; text: string }
    | { type: "paths"; paths: string[] }
    | { type: "mouse"; action: "press" | "release" | "move"; button: MouseButton; col: number; row: number; mods: Mods }
    | { type: "wheel"; col: number; row: number; lines: number; mods: Mods }
    | { type: "focus"; focused: boolean };

export interface GridPoint {
    line: number;
    col: number;
}

export interface FindResult {
    epoch: number;
    current: number | null;
    matches: [GridPoint, GridPoint][];
    more: boolean;
}
