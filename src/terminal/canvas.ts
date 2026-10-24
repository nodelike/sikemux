import { drawGlyph, isDrawnGlyph } from "./boxDrawing";
import {
    COLOR_BACKGROUND,
    COLOR_FOREGROUND,
    COLOR_RGB,
    CURSOR_BEAM,
    CURSOR_BLINKS,
    CURSOR_BLOCK,
    CURSOR_HIDDEN,
    CURSOR_HOLLOW,
    CURSOR_UNDERLINE,
    STYLE_ANY_UNDERLINE,
    STYLE_BOLD,
    STYLE_CURLY_UNDERLINE,
    STYLE_DASHED_UNDERLINE,
    STYLE_DIM,
    STYLE_DOTTED_UNDERLINE,
    STYLE_DOUBLE_UNDERLINE,
    STYLE_HIDDEN,
    STYLE_INVERSE,
    STYLE_ITALIC,
    STYLE_STRIKE,
    type GridPoint,
    type Row,
    type Span,
    type TerminalGrid,
} from "./grid";

// JetBrains Mono's ascender plus descender, as a fraction of the font size.
// Ghostty spaces rows by this, so a row there is 1.32 times the font size.
const FONT_LINE_HEIGHT_EM = 1.32;

export const MATCH_BACKGROUND = "#433866";
export const ACTIVE_MATCH_BACKGROUND = "#a277ff";

export interface TerminalTheme {
    foreground: string;
    /** The theme's own background, for text shown inverted. The pane behind the canvas shows through everywhere else. */
    background: string;
    cursor: string;
    cursorAccent: string;
    selectionBackground: string;
    ansi: readonly string[];
}

export interface Font {
    family: string;
    size: number;
    weight: number;
    boldWeight: number;
}

/** What to draw over the cells besides the cells themselves. */
export interface Decorations {
    /** The line number on the top row. */
    top: number;
    /** First and last selected cell, in order. */
    selection: readonly [GridPoint, GridPoint] | null;
    matches: readonly (readonly [GridPoint, GridPoint])[];
    activeMatch: number | null;
    link: { line: number; from: number; to: number } | null;
    cursor: { visible: boolean; focused: boolean; blinkOn: boolean };
}

interface Drawn {
    row: Row | undefined;
    signature: string;
}

const CUBE = [0, 95, 135, 175, 215, 255];

function hex(rgb: number): string {
    return `#${rgb.toString(16).padStart(6, "0")}`;
}

function parseColor(color: string): number | null {
    const match = /^#([0-9a-f]{6})$/i.exec(color.trim());
    return match ? parseInt(match[1], 16) : null;
}

function dimmed(color: string): string {
    const rgb = parseColor(color);
    if (rgb === null) return color;
    const scale = (channel: number) => Math.round((channel * 2) / 3);
    return hex((scale(rgb >> 16) << 16) | (scale((rgb >> 8) & 0xff) << 8) | scale(rgb & 0xff));
}

function simpleText(text: string): boolean {
    for (let index = 0; index < text.length; index++) {
        const code = text.charCodeAt(index);
        if (code >= 0x2e80 || (code >= 0x2500 && code <= 0x259f) || (code >= 0xd800 && code <= 0xdfff)) return false;
    }
    return true;
}

function overlaps(line: number, from: number, to: number, range: readonly [GridPoint, GridPoint]): [number, number] | null {
    const [start, end] = range;
    if (line < start.line || line > end.line) return null;
    const first = line === start.line ? start.col : 0;
    const last = line === end.line ? end.col : Number.MAX_SAFE_INTEGER;
    const lo = Math.max(first, from);
    const hi = Math.min(last, to);
    return lo <= hi ? [lo, hi] : null;
}

/** Draws a terminal's rows onto one canvas, redrawing only rows that changed. */
export class TerminalCanvas {
    readonly element: HTMLCanvasElement;
    private readonly ctx: CanvasRenderingContext2D;
    private dpr = 1;
    private font: Font = { family: "monospace", size: 13, weight: 400, boldWeight: 700 };
    private theme: TerminalTheme | null = null;
    private colors: string[] = [];
    private palette: ReadonlyMap<number, number> | null = null;
    private rgbCache = new Map<number, string>();
    /** Cell size in device pixels. */
    private cellW = 8;
    private cellH = 17;
    private baseline = 13;
    private advance = 8;
    private letterSpacing = false;
    private drawn: Drawn[] = [];
    private generation = 0;
    private currentFont = "";
    private currentSpacing = "";

    constructor(host: HTMLElement) {
        this.element = document.createElement("canvas");
        this.element.className = "terminal-canvas";
        host.appendChild(this.element);
        const ctx = this.element.getContext("2d", { alpha: true });
        if (!ctx) throw new Error("this window cannot draw a terminal");
        this.ctx = ctx;
        this.letterSpacing = "letterSpacing" in ctx;
    }

    /** Cell size in CSS pixels. */
    get cellWidth(): number {
        return this.cellW / this.dpr;
    }

    get cellHeight(): number {
        return this.cellH / this.dpr;
    }

    setFont(font: Font): void {
        this.font = font;
        this.measure();
    }

    setTheme(theme: TerminalTheme): void {
        this.theme = theme;
        this.buildColors();
    }

    private measure(): void {
        this.dpr = window.devicePixelRatio || 1;
        const ctx = this.ctx;
        ctx.font = this.fontString(false, false);
        this.currentFont = ctx.font;
        const metrics = ctx.measureText("W");
        this.advance = metrics.width;
        this.cellW = Math.max(1, Math.round(metrics.width));
        const ascent = metrics.fontBoundingBoxAscent || this.font.size * this.dpr * 0.8;
        const descent = metrics.fontBoundingBoxDescent || this.font.size * this.dpr * 0.2;
        this.cellH = Math.max(Math.ceil(ascent + descent), Math.round(this.font.size * FONT_LINE_HEIGHT_EM * this.dpr));
        this.baseline = Math.round((this.cellH - (ascent + descent)) / 2 + ascent);
        this.generation++;
    }

    private useFont(bold: boolean, italic: boolean): void {
        const font = this.fontString(bold, italic);
        if (font !== this.currentFont) {
            this.ctx.font = font;
            this.currentFont = font;
        }
    }

    private useSpacing(spacing: string): void {
        if (spacing === this.currentSpacing) return;
        this.ctx.letterSpacing = spacing;
        this.currentSpacing = spacing;
    }

    private fontString(bold: boolean, italic: boolean): string {
        const weight = bold ? this.font.boldWeight : this.font.weight;
        return `${italic ? "italic " : ""}${weight} ${this.font.size * this.dpr}px ${this.font.family}`;
    }

    private buildColors(): void {
        const theme = this.theme;
        if (!theme) return;
        const colors: string[] = [];
        for (let index = 0; index < 16; index++) colors.push(theme.ansi[index] ?? theme.foreground);
        for (let index = 16; index < 232; index++) {
            const cube = index - 16;
            colors.push(hex((CUBE[Math.floor(cube / 36)] << 16) | (CUBE[Math.floor(cube / 6) % 6] << 8) | CUBE[cube % 6]));
        }
        for (let index = 232; index < 256; index++) {
            const level = 8 + (index - 232) * 10;
            colors.push(hex((level << 16) | (level << 8) | level));
        }
        colors.push(theme.foreground, theme.background, theme.cursor);
        for (let index = 0; index < 8; index++) colors.push(dimmed(colors[index]));
        colors.push(theme.foreground, dimmed(theme.foreground));
        if (this.palette) for (const [index, rgb] of this.palette) if (index < colors.length) colors[index] = hex(rgb);
        this.colors = colors;
        this.generation++;
    }

    private color(value: number): string {
        if (value >= COLOR_RGB) {
            let cached = this.rgbCache.get(value);
            if (!cached) {
                cached = hex(value & 0xffffff);
                if (this.rgbCache.size > 4096) this.rgbCache.clear();
                this.rgbCache.set(value, cached);
            }
            return cached;
        }
        return this.colors[value] ?? this.colors[COLOR_FOREGROUND] ?? "#ffffff";
    }

    /** The columns and rows that fit the canvas's box. */
    fit(width: number, height: number): { cols: number; rows: number } {
        if (window.devicePixelRatio !== this.dpr) this.measure();
        const deviceWidth = Math.round(width * this.dpr);
        const deviceHeight = Math.round(height * this.dpr);
        if (this.element.width !== deviceWidth || this.element.height !== deviceHeight) {
            this.element.width = deviceWidth;
            this.element.height = deviceHeight;
            this.generation++;
        }
        return {
            cols: Math.max(2, Math.floor(deviceWidth / this.cellW)),
            rows: Math.max(1, Math.floor(deviceHeight / this.cellH)),
        };
    }

    /** The cell under a point, in CSS pixels from the canvas's corner. */
    cellAt(x: number, y: number): { col: number; row: number; right: boolean } {
        const deviceX = x * this.dpr;
        const col = Math.max(0, Math.floor(deviceX / this.cellW));
        return { col, row: Math.max(0, Math.floor((y * this.dpr) / this.cellH)), right: deviceX - col * this.cellW > this.cellW / 2 };
    }

    /** Forgets what was drawn, so the next paint draws every row. */
    invalidate(): void {
        this.generation++;
    }

    paint(grid: TerminalGrid, rows: number, decorations: Decorations): void {
        if (!this.theme) return;
        if (grid.palette !== this.palette) {
            this.palette = grid.palette;
            this.buildColors();
        }
        const cols = grid.cols;
        const header = grid.header;
        const cursorLine = header.historyEnd + header.cursor.row;
        if (this.drawn.length !== rows) this.drawn = new Array<Drawn>(rows).fill({ row: undefined, signature: "" });
        this.shiftDrawn(grid, rows, decorations.top);
        for (let screenRow = 0; screenRow < rows; screenRow++) {
            const line = decorations.top + screenRow;
            const row = grid.line(line);
            const signature = this.signature(line, cols, decorations, line === cursorLine ? header.cursor : null);
            const drawn = this.drawn[screenRow];
            if (drawn.row === row && drawn.signature === signature) continue;
            this.drawn[screenRow] = { row, signature };
            this.drawRow(screenRow, line, row, cols, decorations, line === cursorLine ? header.cursor : null);
        }
    }

    /** Rows that scrolled are moved on the canvas rather than drawn again. */
    private shiftDrawn(grid: TerminalGrid, rows: number, top: number): void {
        const first = grid.line(top);
        if (!first || this.drawn[0]?.row === first) return;
        const blank: Drawn = { row: undefined, signature: "" };
        const move = (from: number, to: number, count: number) => {
            const height = count * this.cellH;
            this.ctx.save();
            this.ctx.globalCompositeOperation = "copy";
            this.ctx.drawImage(this.element, 0, from * this.cellH, this.element.width, height, 0, to * this.cellH, this.element.width, height);
            this.ctx.restore();
        };
        for (let shift = 1; shift < rows - 1; shift++) {
            if (this.drawn[shift].row === first && this.drawn[shift + 1].row === grid.line(top + 1)) {
                move(shift, 0, rows - shift);
                this.drawn = [...this.drawn.slice(shift), ...new Array<Drawn>(shift).fill(blank)];
                return;
            }
            const drawnTop = this.drawn[0].row;
            if (drawnTop && drawnTop === grid.line(top + shift) && this.drawn[1].row === grid.line(top + shift + 1)) {
                move(0, shift, rows - shift);
                this.drawn = [...new Array<Drawn>(shift).fill(blank), ...this.drawn.slice(0, rows - shift)];
                return;
            }
        }
    }

    private signature(line: number, cols: number, decorations: Decorations, cursor: { col: number; shape: number } | null): string {
        let signature = `${this.generation}`;
        if (decorations.selection) {
            const covered = overlaps(line, 0, cols, decorations.selection);
            if (covered) signature += `s${covered[0]}-${covered[1]}`;
        }
        decorations.matches.forEach((match, index) => {
            const covered = overlaps(line, 0, cols, match);
            if (covered) signature += `m${covered[0]}-${covered[1]}${index === decorations.activeMatch ? "a" : ""}`;
        });
        if (decorations.link?.line === line) signature += `l${decorations.link.from}-${decorations.link.to}`;
        if (cursor && decorations.cursor.visible) {
            const on = !(cursor.shape & CURSOR_BLINKS) || !decorations.cursor.focused || decorations.cursor.blinkOn;
            signature += `c${cursor.col}.${cursor.shape}.${decorations.cursor.focused ? 1 : 0}.${on ? 1 : 0}`;
        }
        return signature;
    }

    private drawRow(
        screenRow: number,
        line: number,
        row: Row | undefined,
        cols: number,
        decorations: Decorations,
        cursor: { col: number; shape: number } | null,
    ): void {
        const ctx = this.ctx;
        const theme = this.theme!;
        const y = screenRow * this.cellH;
        ctx.clearRect(0, y, this.element.width, this.cellH);
        const spans = row?.spans ?? [];

        for (const span of spans) {
            const { bg } = this.spanColors(span);
            if (bg === null) continue;
            ctx.fillStyle = bg;
            ctx.fillRect(span.col * this.cellW, y, span.width * this.cellW, this.cellH);
        }
        const highlight = (range: readonly [GridPoint, GridPoint], color: string) => {
            const covered = overlaps(line, 0, cols - 1, range);
            if (!covered) return;
            ctx.fillStyle = color;
            ctx.fillRect(covered[0] * this.cellW, y, (covered[1] - covered[0] + 1) * this.cellW, this.cellH);
        };
        decorations.matches.forEach((match, index) =>
            highlight(match, index === decorations.activeMatch ? ACTIVE_MATCH_BACKGROUND : MATCH_BACKGROUND),
        );
        if (decorations.selection) highlight(decorations.selection, theme.selectionBackground);

        for (const span of spans) this.drawSpan(span, y);

        if (decorations.link?.line === line) {
            ctx.fillStyle = theme.foreground;
            const thickness = Math.max(1, Math.round(this.dpr));
            ctx.fillRect(
                decorations.link.from * this.cellW,
                y + this.baseline + thickness,
                (decorations.link.to - decorations.link.from + 1) * this.cellW,
                thickness,
            );
        }
        if (cursor && decorations.cursor.visible) this.drawCursor(row, cursor, y, decorations.cursor);
    }

    private spanColors(span: Span): { fg: string; bg: string | null } {
        const style = span.style;
        let fg = style.fg;
        let bg: number | null = style.bg === COLOR_BACKGROUND ? null : style.bg;
        if (style.flags & STYLE_INVERSE) {
            const swapped = fg;
            fg = bg ?? COLOR_BACKGROUND;
            bg = swapped;
        }
        return { fg: this.color(fg), bg: bg === null ? null : this.color(bg) };
    }

    private drawSpan(span: Span, y: number): void {
        const style = span.style;
        if (style.flags & STYLE_HIDDEN) return;
        const ctx = this.ctx;
        const { fg } = this.spanColors(span);
        this.useFont((style.flags & STYLE_BOLD) !== 0, (style.flags & STYLE_ITALIC) !== 0);
        ctx.fillStyle = fg;
        ctx.globalAlpha = style.flags & STYLE_DIM ? 0.5 : 1;
        const baseline = y + this.baseline;
        const x = span.col * this.cellW;
        if (!span.wide && !span.cluster && simpleText(span.text)) {
            this.fillGridText(span.text, x, baseline);
        } else if (span.cluster) {
            this.fillCell(span.text, x, y, baseline, span.wide);
        } else {
            let col = span.col;
            let run = "";
            let runCol = col;
            for (const character of span.text) {
                const code = character.codePointAt(0)!;
                if (!span.wide && simpleText(character)) {
                    if (!run) runCol = col;
                    run += character;
                } else {
                    if (run) this.fillGridText(run, runCol * this.cellW, baseline);
                    run = "";
                    this.fillCell(character, col * this.cellW, y, baseline, span.wide, code);
                }
                col += span.wide ? 2 : 1;
            }
            if (run) this.fillGridText(run, runCol * this.cellW, baseline);
        }
        this.decorate(span, x, y, fg);
        ctx.globalAlpha = 1;
    }

    /** Text whose characters each take one cell, kept on the cell grid. */
    private fillGridText(text: string, x: number, baseline: number): void {
        const ctx = this.ctx;
        if (this.letterSpacing) {
            this.useSpacing(`${this.cellW - this.advance}px`);
            ctx.fillText(text, x, baseline);
            return;
        }
        ctx.save();
        ctx.setTransform(this.cellW / this.advance, 0, 0, 1, x, 0);
        ctx.fillText(text, 0, baseline);
        ctx.restore();
    }

    private fillCell(text: string, x: number, y: number, baseline: number, wide: boolean, code = text.codePointAt(0) ?? 0): void {
        const width = wide ? this.cellW * 2 : this.cellW;
        if (isDrawnGlyph(code) && drawGlyph(this.ctx, code, x, y, width, this.cellH, Math.max(1, Math.round(this.dpr)))) return;
        const ctx = this.ctx;
        if (this.letterSpacing) this.useSpacing("0px");
        const measured = ctx.measureText(text).width;
        const offset = measured < width ? (width - measured) / 2 : 0;
        if (measured > width) {
            ctx.save();
            ctx.setTransform(width / measured, 0, 0, 1, x, 0);
            ctx.fillText(text, 0, baseline);
            ctx.restore();
            return;
        }
        ctx.fillText(text, x + offset, baseline);
    }

    private decorate(span: Span, x: number, y: number, fg: string): void {
        const flags = span.style.flags;
        if (!(flags & (STYLE_ANY_UNDERLINE | STYLE_STRIKE))) return;
        const ctx = this.ctx;
        const width = span.width * this.cellW;
        const thickness = Math.max(1, Math.round(this.dpr));
        ctx.fillStyle = span.style.underline === null ? fg : this.color(span.style.underline);
        const under = y + Math.min(this.cellH - thickness, this.baseline + thickness);
        if (flags & STYLE_DOUBLE_UNDERLINE) {
            ctx.fillRect(x, under - thickness, width, thickness);
            ctx.fillRect(x, under + thickness, width, thickness);
        } else if (flags & STYLE_CURLY_UNDERLINE) {
            ctx.save();
            ctx.strokeStyle = ctx.fillStyle;
            ctx.lineWidth = thickness;
            ctx.beginPath();
            const amplitude = thickness * 1.5;
            for (let offset = 0; offset <= width; offset += thickness) {
                const wave = Math.sin((offset / (this.cellW / 2)) * Math.PI) * amplitude;
                if (offset === 0) ctx.moveTo(x, under + wave);
                else ctx.lineTo(x + offset, under + wave);
            }
            ctx.stroke();
            ctx.restore();
        } else if (flags & (STYLE_DOTTED_UNDERLINE | STYLE_DASHED_UNDERLINE)) {
            const dash = flags & STYLE_DOTTED_UNDERLINE ? thickness : thickness * 3;
            for (let offset = 0; offset < width; offset += dash * 2) ctx.fillRect(x + offset, under, Math.min(dash, width - offset), thickness);
        } else if (flags & STYLE_ANY_UNDERLINE) {
            ctx.fillRect(x, under, width, thickness);
        }
        ctx.fillStyle = fg;
        if (flags & STYLE_STRIKE) ctx.fillRect(x, y + Math.round(this.cellH / 2), width, thickness);
    }

    private drawCursor(row: Row | undefined, cursor: { col: number; shape: number }, y: number, state: Decorations["cursor"]): void {
        const theme = this.theme!;
        const shape = cursor.shape & ~CURSOR_BLINKS;
        if (shape === CURSOR_HIDDEN) return;
        if (state.focused && cursor.shape & CURSOR_BLINKS && !state.blinkOn) return;
        const ctx = this.ctx;
        const x = cursor.col * this.cellW;
        const span = row?.spans.find((candidate) => cursor.col >= candidate.col && cursor.col < candidate.col + candidate.width);
        const width = span?.wide ? this.cellW * 2 : this.cellW;
        const thickness = Math.max(1, Math.round(this.dpr));
        ctx.fillStyle = theme.cursor;
        if (!state.focused || shape === CURSOR_HOLLOW) {
            ctx.fillRect(x, y, width, thickness);
            ctx.fillRect(x, y + this.cellH - thickness, width, thickness);
            ctx.fillRect(x, y, thickness, this.cellH);
            ctx.fillRect(x + width - thickness, y, thickness, this.cellH);
            return;
        }
        if (shape === CURSOR_BEAM) {
            ctx.fillRect(x, y, Math.max(thickness, Math.round(this.dpr * 2)), this.cellH);
            return;
        }
        if (shape === CURSOR_UNDERLINE) {
            ctx.fillRect(x, y + this.cellH - thickness * 2, width, thickness * 2);
            return;
        }
        if (shape === CURSOR_BLOCK) {
            ctx.fillRect(x, y, width, this.cellH);
            if (!span) return;
            const text = span.cluster ? span.text : [...span.text][Math.floor((cursor.col - span.col) / (span.wide ? 2 : 1))];
            if (!text || text === " ") return;
            this.useFont((span.style.flags & STYLE_BOLD) !== 0, (span.style.flags & STYLE_ITALIC) !== 0);
            ctx.fillStyle = theme.cursorAccent;
            this.fillCell(text, x, y, y + this.baseline, span.wide);
        }
    }

    dispose(): void {
        this.element.remove();
    }
}
