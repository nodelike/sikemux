import { invokeCommand as invoke } from "../api/invoke";
import { openUrl } from "../api/system";
import { copyText, readClipboardText } from "../lib/clipboard";
import { IS_MACOS } from "../lib/platform";
import { safeWebUrl } from "../lib/urls";
import { performanceTelemetry } from "../lib/performance";
import { TerminalCanvas, type Decorations, type Font, type TerminalTheme } from "./canvas";
import {
    CURSOR_BLINKS,
    MODE_ALT_SCREEN,
    MODE_ALTERNATE_SCROLL,
    MODE_MOUSE_ANY,
    MODE_MOUSE_DRAG,
    MODE_MOUSE_MOTION,
    TerminalGrid,
    decodeFrame,
    decodeLines,
    rowCells,
    type FindResult,
    type GridPoint,
    type Mods,
    type MouseButton,
    type TerminalInput,
} from "./grid";
import { isTerminalFindShortcut, type TerminalSearchOptions } from "./interactions";
import type { PtyAttachment } from "./ptyController";

const HISTORY_PAGE = 256;
const BLINK_MS = 600;
const EXPENSIVE_PAINT_MS = 8;
const TYPING_MS = 250;
const MODE_FOCUS_REPORTS = 1 << 6;
const WORD_SEPARATORS = " ()[]{}<>'\"`,;│|";
const URL_PATTERN = /https?:\/\/[^\s"'<>`]+/g;

const NAMED_KEYS = new Set([
    "Enter",
    "Backspace",
    "Tab",
    "Escape",
    "ArrowUp",
    "ArrowDown",
    "ArrowLeft",
    "ArrowRight",
    "Home",
    "End",
    "PageUp",
    "PageDown",
    "Insert",
    "Delete",
    ...Array.from({ length: 12 }, (_, index) => `F${index + 1}`),
]);

/** The character a key stands for with option held, which macOS would otherwise turn into another letter. */
function baseKey(event: KeyboardEvent): string {
    const code = event.code;
    if (code.startsWith("Key")) return event.shiftKey ? code.slice(3) : code.slice(3).toLowerCase();
    if (code.startsWith("Digit")) return code.slice(5);
    const punctuation: Record<string, string> = {
        Minus: "-",
        Equal: "=",
        BracketLeft: "[",
        BracketRight: "]",
        Backslash: "\\",
        Semicolon: ";",
        Quote: "'",
        Comma: ",",
        Period: ".",
        Slash: "/",
        Backquote: "`",
        Space: " ",
    };
    return punctuation[code] ?? event.key;
}

function modsOf(event: KeyboardEvent | MouseEvent): Mods {
    return { shift: event.shiftKey, alt: event.altKey, ctrl: event.ctrlKey, meta: event.metaKey };
}

function before(a: GridPoint, b: GridPoint): boolean {
    return a.line < b.line || (a.line === b.line && a.col < b.col);
}

export interface SearchResults {
    resultIndex: number;
    resultCount: number;
}

export interface TerminalViewOptions {
    host: HTMLElement;
    pid: number;
    font: Font;
    theme: TerminalTheme;
    input(input: TerminalInput): void;
    onFindRequest(seed: string): void;
    onSearchResults(results: SearchResults): void;
    onExit(): void;
}

/**
 * One pane's terminal: draws the core's frames onto a canvas, keeps the
 * history it scrolled to, and sends what the person does back as input.
 */
export class TerminalView {
    readonly element: HTMLDivElement;
    private readonly canvas: TerminalCanvas;
    private readonly textarea: HTMLTextAreaElement;
    private readonly scrollbar: HTMLDivElement;
    private readonly exitedNote: HTMLDivElement;
    private readonly grid = new TerminalGrid();
    private attachment: PtyAttachment | null = null;
    private size = { cols: 0, rows: 0 };
    /** The line on the top row, or null to follow the newest output. */
    private top: number | null = null;
    private visible = true;
    private focused = false;
    private blinkOn = true;
    private blinkTimer: number | null = null;
    private frameRequest: number | null = null;
    private ackedFrame = -1;
    private ackTimer: number | null = null;
    private lastInput = -Infinity;
    private wheelRemainder = 0;
    private composing = false;
    private fetching = new Set<number>();
    private anchor: GridPoint | null = null;
    private head: GridPoint | null = null;
    private dragging = false;
    private dragMoved = false;
    private mouseButton: MouseButton | null = null;
    private lastMotion = "";
    private link: { line: number; from: number; to: number; uri: string } | null = null;
    private matches: [GridPoint, GridPoint][] = [];
    private activeMatch: number | null = null;
    private searchEpoch = -1;
    private disposed = false;
    private readonly cleanups: Array<() => void> = [];

    constructor(private readonly options: TerminalViewOptions) {
        const element = document.createElement("div");
        element.className = "terminal-surface";
        element.tabIndex = -1;
        this.element = element;
        options.host.appendChild(element);
        this.canvas = new TerminalCanvas(element);
        this.canvas.setFont(options.font);
        this.canvas.setTheme(options.theme);

        const textarea = document.createElement("textarea");
        textarea.className = "terminal-input";
        textarea.setAttribute("autocorrect", "off");
        textarea.setAttribute("autocapitalize", "off");
        textarea.setAttribute("aria-label", "Terminal input");
        textarea.spellcheck = false;
        element.appendChild(textarea);
        this.textarea = textarea;

        this.scrollbar = document.createElement("div");
        this.scrollbar.className = "terminal-scrollbar";
        element.appendChild(this.scrollbar);

        this.exitedNote = document.createElement("div");
        this.exitedNote.className = "terminal-exited";
        this.exitedNote.textContent = "[process exited]";
        this.exitedNote.hidden = true;
        element.appendChild(this.exitedNote);

        this.listen(textarea, "keydown", (event) => this.onKeyDown(event as KeyboardEvent));
        this.listen(textarea, "input", () => this.onTextInput());
        this.listen(textarea, "compositionstart", () => (this.composing = true));
        this.listen(textarea, "compositionend", (event) => this.onCompositionEnd(event as CompositionEvent));
        this.listen(textarea, "paste", (event) => this.onPaste(event as ClipboardEvent));
        this.listen(textarea, "copy", (event) => this.onCopy(event as ClipboardEvent));
        this.listen(textarea, "focus", () => this.setFocused(true));
        this.listen(textarea, "blur", () => this.setFocused(false));
        this.listen(element, "pointerdown", (event) => this.onPointerDown(event as PointerEvent));
        this.listen(element, "pointermove", (event) => this.onPointerMove(event as PointerEvent));
        this.listen(element, "pointerup", (event) => this.onPointerUp(event as PointerEvent));
        this.listen(element, "wheel", (event) => this.onWheel(event as WheelEvent), { passive: false });
    }

    private listen(target: EventTarget, type: string, handler: (event: Event) => void, options?: AddEventListenerOptions): void {
        target.addEventListener(type, handler, options);
        this.cleanups.push(() => target.removeEventListener(type, handler, options));
    }

    get pid(): number {
        return this.options.pid;
    }

    /** The columns and rows the pane has room for now. */
    fit(): { cols: number; rows: number } {
        const width = this.element.clientWidth;
        const height = this.element.clientHeight;
        if (width === 0 || height === 0) return this.size;
        const size = this.canvas.fit(width, height);
        const changed = size.cols !== this.size.cols || size.rows !== this.size.rows;
        this.size = size;
        if (changed) this.canvas.invalidate();
        this.schedulePaint();
        return size;
    }

    setFont(font: Font): void {
        this.canvas.setFont(font);
        this.fit();
    }

    setTheme(theme: TerminalTheme): void {
        this.canvas.setTheme(theme);
        this.schedulePaint();
    }

    setVisible(visible: boolean): void {
        this.visible = visible;
        if (visible) {
            this.canvas.invalidate();
            this.schedulePaint();
        }
    }

    /** Starts drawing a stream: its first frame, then the frames the attachment releases. */
    start(attachment: PtyAttachment): void {
        this.attachment = attachment;
        this.receive(attachment.frame);
        attachment.activate();
    }

    /** A frame from the core, or an empty chunk once the process is gone. */
    receive(chunk: Uint8Array): void {
        if (this.disposed) return;
        if (chunk.length === 0) {
            this.exitedNote.hidden = false;
            this.options.onExit();
            return;
        }
        let frame;
        try {
            frame = decodeFrame(chunk);
        } catch (error) {
            performanceTelemetry.incrementCounter("terminal.frame-errors");
            console.warn("terminal frame could not be read", error);
            return;
        }
        const epoch = this.grid.header.epoch;
        this.grid.apply(frame);
        const header = this.grid.header;
        if (header.epoch !== epoch) {
            this.top = null;
            this.fetching.clear();
            if (this.searchEpoch !== header.epoch) this.dropMatches();
            this.anchor = this.head = null;
        }
        if (this.top !== null && this.top < this.grid.historyStart) this.top = this.grid.historyStart;
        if (this.top !== null && this.top >= header.historyEnd) this.top = null;
        this.schedulePaint();
    }

    private get viewTop(): number {
        return this.top ?? this.grid.header.historyEnd;
    }

    private schedulePaint(): void {
        if (this.frameRequest !== null || !this.visible || this.disposed) return;
        this.frameRequest = requestAnimationFrame(() => {
            this.frameRequest = null;
            this.paint();
        });
    }

    private paint(): void {
        if (this.disposed || !this.visible || this.grid.header.rows === 0) return;
        const started = performance.now();
        this.ensureHistory();
        const decorations: Decorations = {
            top: this.viewTop,
            selection: this.selectionRange(),
            matches: this.matches,
            activeMatch: this.activeMatch,
            link: this.link,
            cursor: { visible: this.top === null, focused: this.focused, blinkOn: this.blinkOn },
        };
        this.canvas.paint(this.grid, this.grid.header.rows, decorations);
        this.placeTextarea();
        this.drawScrollbar();
        const cost = performance.now() - started;
        performanceTelemetry.recordLatency("terminal.paint", cost);
        const frame = this.grid.header.frame;
        if (frame === this.ackedFrame || this.ackTimer !== null) return;
        // A flood that changes every row costs a long paint each frame. Asking for
        // the next one only after as long again keeps half the page's time free,
        // except just after the person typed, whose echo should show at once.
        if (cost < EXPENSIVE_PAINT_MS || performance.now() - this.lastInput < TYPING_MS) this.ack();
        else this.ackTimer = window.setTimeout(() => this.ack(), cost);
    }

    private ack(): void {
        this.ackTimer = null;
        const frame = this.grid.header.frame;
        if (frame === this.ackedFrame || this.disposed) return;
        this.ackedFrame = frame;
        this.attachment?.ack(frame);
    }

    private placeTextarea(): void {
        const { row, col } = this.grid.header.cursor;
        this.textarea.style.transform = `translate(${col * this.canvas.cellWidth}px, ${row * this.canvas.cellHeight}px)`;
        this.textarea.style.height = `${this.canvas.cellHeight}px`;
    }

    private drawScrollbar(): void {
        const header = this.grid.header;
        const total = header.historyLen + header.rows;
        if (header.historyLen === 0 || header.modes & MODE_ALT_SCREEN) {
            this.scrollbar.hidden = true;
            return;
        }
        this.scrollbar.hidden = false;
        const height = this.element.clientHeight;
        const thumb = Math.max(20, (header.rows / total) * height);
        const position = (this.viewTop - this.grid.historyStart) / header.historyLen;
        this.scrollbar.style.height = `${thumb}px`;
        this.scrollbar.style.transform = `translateY(${position * (height - thumb)}px)`;
        this.scrollbar.dataset.scrolled = this.top === null ? "false" : "true";
    }

    /** Fetches the history lines on screen that this pane does not have yet. */
    private ensureHistory(): void {
        if (this.top === null) return;
        const end = Math.min(this.top + this.grid.header.rows, this.grid.header.historyEnd);
        for (let line = this.top; line < end; line++) {
            if (this.grid.hasLine(line)) continue;
            const page = Math.floor(line / HISTORY_PAGE) * HISTORY_PAGE;
            line = page + HISTORY_PAGE - 1;
            if (this.fetching.has(page)) continue;
            this.fetching.add(page);
            void invoke<ArrayBuffer>("pty_lines", { id: this.pid, start: page, count: HISTORY_PAGE })
                .then((body) => {
                    if (this.disposed) return;
                    this.grid.applyLines(decodeLines(new Uint8Array(body)));
                    this.fetching.delete(page);
                    this.schedulePaint();
                })
                .catch(() => this.fetching.delete(page));
        }
    }

    private scrollBy(lines: number): void {
        const header = this.grid.header;
        if (header.modes & MODE_ALT_SCREEN) return;
        const next = Math.max(this.grid.historyStart, Math.min(header.historyEnd, this.viewTop + lines));
        this.top = next >= header.historyEnd ? null : next;
        this.schedulePaint();
    }

    scrollToBottom(): void {
        if (this.top === null) return;
        this.top = null;
        this.schedulePaint();
    }

    private send(input: TerminalInput): void {
        this.lastInput = performance.now();
        if (this.ackTimer !== null) {
            window.clearTimeout(this.ackTimer);
            this.ack();
        }
        this.scrollToBottom();
        this.options.input(input);
    }

    // Keyboard

    private onKeyDown(event: KeyboardEvent): void {
        if (event.isComposing || event.keyCode === 229) return;
        if (isTerminalFindShortcut(event, IS_MACOS)) {
            event.preventDefault();
            event.stopPropagation();
            this.options.onFindRequest(this.getSelection());
            return;
        }
        if (event.code === "KeyR" && event.metaKey && event.altKey && !event.ctrlKey && !event.shiftKey) {
            event.preventDefault();
            event.stopPropagation();
            void invoke("pty_reset_modes", { id: this.pid }).catch(() => {});
            return;
        }
        const command = IS_MACOS ? event.metaKey : event.ctrlKey && event.shiftKey;
        if (command) {
            const key = event.key.toLowerCase();
            if (key === "c" && this.selectionRange()) {
                event.preventDefault();
                void this.copySelection();
                return;
            }
            if (key === "v") return;
            if (key === "a") {
                event.preventDefault();
                this.selectAll();
                return;
            }
            if (IS_MACOS && (event.key === "ArrowLeft" || event.key === "ArrowRight" || event.key === "Backspace")) {
                event.preventDefault();
                this.send({ type: "key", key: event.key, mods: modsOf(event) });
            }
            return;
        }
        if (event.shiftKey && (event.key === "PageUp" || event.key === "PageDown") && !(this.grid.header.modes & MODE_ALT_SCREEN)) {
            event.preventDefault();
            this.scrollBy((event.key === "PageUp" ? -1 : 1) * this.grid.header.rows);
            return;
        }
        if (NAMED_KEYS.has(event.key)) {
            event.preventDefault();
            this.send({ type: "key", key: event.key, mods: modsOf(event) });
            return;
        }
        if ([...event.key].length !== 1) return;
        if (event.ctrlKey || (event.altKey && IS_MACOS)) {
            event.preventDefault();
            this.send({ type: "key", key: event.altKey && IS_MACOS ? baseKey(event) : event.key, mods: modsOf(event) });
        }
    }

    private onTextInput(): void {
        if (this.composing) return;
        const text = this.textarea.value;
        this.textarea.value = "";
        if (text) this.send({ type: "text", text });
    }

    private onCompositionEnd(event: CompositionEvent): void {
        this.composing = false;
        this.textarea.value = "";
        if (event.data) this.send({ type: "text", text: event.data });
    }

    private onPaste(event: ClipboardEvent): void {
        event.preventDefault();
        const text = event.clipboardData?.getData("text/plain");
        if (text) this.send({ type: "paste", text });
    }

    private onCopy(event: ClipboardEvent): void {
        if (!this.selectionRange()) return;
        event.preventDefault();
        void this.copySelection();
    }

    private setFocused(focused: boolean): void {
        this.focused = focused;
        this.blinkOn = true;
        if (this.blinkTimer !== null) window.clearInterval(this.blinkTimer);
        this.blinkTimer = null;
        if (focused && this.grid.header.cursor.shape & CURSOR_BLINKS) {
            this.blinkTimer = window.setInterval(() => {
                this.blinkOn = !this.blinkOn;
                this.schedulePaint();
            }, BLINK_MS);
        }
        if (this.grid.header.modes & MODE_FOCUS_REPORTS) this.options.input({ type: "focus", focused });
        this.schedulePaint();
    }

    focus(): void {
        this.textarea.focus({ preventScroll: true });
    }

    // Mouse

    private cellAt(event: MouseEvent): { col: number; row: number; right: boolean } {
        const rect = this.element.getBoundingClientRect();
        const cell = this.canvas.cellAt(event.clientX - rect.left, event.clientY - rect.top);
        return {
            col: Math.min(cell.col, Math.max(0, this.grid.cols - 1)),
            row: Math.min(cell.row, Math.max(0, this.grid.header.rows - 1)),
            right: cell.right,
        };
    }

    private reportsMouse(event: MouseEvent): boolean {
        return (this.grid.header.modes & MODE_MOUSE_ANY) !== 0 && !event.shiftKey && !(IS_MACOS && event.altKey);
    }

    private onPointerDown(event: PointerEvent): void {
        if ((event.target as HTMLElement).closest(".terminal-find")) return;
        this.focus();
        if (event.button === 2) return;
        event.preventDefault();
        const cell = this.cellAt(event);
        if (this.reportsMouse(event)) {
            const button: MouseButton = event.button === 1 ? "middle" : "left";
            this.mouseButton = button;
            this.element.setPointerCapture(event.pointerId);
            this.options.input({ type: "mouse", action: "press", button, col: cell.col, row: cell.row, mods: modsOf(event) });
            return;
        }
        if (event.button !== 0) return;
        const point = { line: this.viewTop + cell.row, col: cell.col };
        this.element.setPointerCapture(event.pointerId);
        this.dragging = true;
        this.dragMoved = false;
        if (event.detail === 2) this.selectWord(point);
        else if (event.detail >= 3) this.selectLine(point);
        else if (event.shiftKey && this.anchor) this.head = point;
        else {
            this.anchor = { line: point.line, col: cell.right ? point.col + 1 : point.col };
            this.head = this.anchor;
        }
        this.schedulePaint();
    }

    private onPointerMove(event: PointerEvent): void {
        const cell = this.cellAt(event);
        if (this.mouseButton !== null || (this.reportsMouse(event) && this.grid.header.modes & MODE_MOUSE_MOTION)) {
            const key = `${cell.col}:${cell.row}`;
            if (key === this.lastMotion) return;
            this.lastMotion = key;
            const button = this.mouseButton ?? "none";
            if (button !== "none" && !(this.grid.header.modes & (MODE_MOUSE_DRAG | MODE_MOUSE_MOTION))) return;
            this.options.input({ type: "mouse", action: "move", button, col: cell.col, row: cell.row, mods: modsOf(event) });
            return;
        }
        if (this.dragging) {
            const rect = this.element.getBoundingClientRect();
            if (event.clientY < rect.top) this.scrollBy(-1);
            else if (event.clientY > rect.bottom) this.scrollBy(1);
            const head = { line: this.viewTop + cell.row, col: cell.right ? cell.col + 1 : cell.col };
            if (this.head && (head.line !== this.head.line || head.col !== this.head.col)) {
                this.head = head;
                this.dragMoved = true;
                this.schedulePaint();
            }
            return;
        }
        this.hoverLink(this.viewTop + cell.row, cell.col);
    }

    private onPointerUp(event: PointerEvent): void {
        if (this.element.hasPointerCapture(event.pointerId)) this.element.releasePointerCapture(event.pointerId);
        if (this.mouseButton !== null) {
            const cell = this.cellAt(event);
            this.options.input({ type: "mouse", action: "release", button: this.mouseButton, col: cell.col, row: cell.row, mods: modsOf(event) });
            this.mouseButton = null;
            return;
        }
        if (!this.dragging) return;
        this.dragging = false;
        if (!this.dragMoved && event.detail === 1 && this.link) {
            const url = safeWebUrl(this.link.uri);
            this.anchor = this.head = null;
            this.schedulePaint();
            if (url) void openUrl(url).catch((error) => console.warn("open terminal link failed", error));
        }
    }

    private onWheel(event: WheelEvent): void {
        event.preventDefault();
        const cellHeight = this.canvas.cellHeight || 16;
        const delta =
            event.deltaMode === 1
                ? event.deltaY * cellHeight
                : event.deltaMode === 2
                  ? event.deltaY * cellHeight * this.grid.header.rows
                  : event.deltaY;
        this.wheelRemainder += delta / cellHeight;
        const lines = Math.trunc(this.wheelRemainder);
        if (lines === 0) return;
        this.wheelRemainder -= lines;
        const modes = this.grid.header.modes;
        if (this.reportsMouse(event) || (modes & MODE_ALT_SCREEN && modes & MODE_ALTERNATE_SCROLL)) {
            const cell = this.cellAt(event);
            this.options.input({ type: "wheel", col: cell.col, row: cell.row, lines, mods: modsOf(event) });
            return;
        }
        this.scrollBy(lines);
    }

    private hoverLink(line: number, col: number): void {
        const found = this.linkAt(line, col);
        const same = found?.line === this.link?.line && found?.from === this.link?.from && found?.uri === this.link?.uri;
        if (same) return;
        this.link = found;
        this.element.classList.toggle("terminal-link-hover", found !== null);
        this.schedulePaint();
    }

    private linkAt(line: number, col: number): { line: number; from: number; to: number; uri: string } | null {
        const row = this.grid.line(line);
        if (!row) return null;
        const span = row.spans.find((candidate) => col >= candidate.col && col < candidate.col + candidate.width);
        if (span?.style.link) {
            let from = span.col;
            let to = span.col + span.width - 1;
            for (const other of row.spans) {
                if (other.style.link !== span.style.link) continue;
                if (other.col + other.width === from) from = other.col;
                if (other.col === to + 1) to = other.col + other.width - 1;
            }
            return { line, from, to, uri: span.style.link };
        }
        const cells = rowCells(row, this.grid.cols);
        const text = cells.join("");
        const offsets: number[] = [];
        cells.forEach((cell, index) => {
            for (let unit = 0; unit < cell.length; unit++) offsets.push(index);
        });
        for (const match of text.matchAll(URL_PATTERN)) {
            const uri = match[0].replace(/[.,;:!?)\]}]+$/, "");
            const from = offsets[match.index] ?? 0;
            const to = offsets[match.index + uri.length - 1] ?? from;
            if (col >= from && col <= to) return { line, from, to, uri };
        }
        return null;
    }

    // Selection

    private selectionRange(): readonly [GridPoint, GridPoint] | null {
        if (!this.anchor || !this.head) return null;
        if (this.anchor.line === this.head.line && this.anchor.col === this.head.col) return null;
        const [start, end] = before(this.head, this.anchor) ? [this.head, this.anchor] : [this.anchor, this.head];
        if (end.col === 0) {
            if (end.line === start.line) return null;
            return [start, { line: end.line - 1, col: Math.max(0, this.grid.cols - 1) }];
        }
        return [start, { line: end.line, col: end.col - 1 }];
    }

    private selectWord(point: GridPoint): void {
        const row = this.grid.line(point.line);
        if (!row) return;
        const cells = rowCells(row, this.grid.cols);
        const isWord = (cell: string | undefined) => cell !== undefined && cell !== "" && !WORD_SEPARATORS.includes(cell);
        if (!isWord(cells[point.col])) {
            this.anchor = point;
            this.head = { line: point.line, col: point.col + 1 };
            return;
        }
        let from = point.col;
        let to = point.col;
        while (from > 0 && (isWord(cells[from - 1]) || cells[from - 1] === "")) from--;
        while (to + 1 < cells.length && (isWord(cells[to + 1]) || cells[to + 1] === "")) to++;
        this.anchor = { line: point.line, col: from };
        this.head = { line: point.line, col: to + 1 };
    }

    private selectLine(point: GridPoint): void {
        let first = point.line;
        while (this.grid.line(first - 1)?.wrapped) first--;
        let last = point.line;
        while (this.grid.line(last)?.wrapped) last++;
        this.anchor = { line: first, col: 0 };
        this.head = { line: last, col: this.grid.cols };
    }

    /** The selected text from the lines this pane has; the core's copy is exact. */
    getSelection(): string {
        const range = this.selectionRange();
        if (!range) return "";
        const [start, end] = range;
        const cols = this.grid.cols;
        let text = "";
        for (let line = start.line; line <= end.line; line++) {
            const row = this.grid.line(line);
            const cells = row ? rowCells(row, cols) : [];
            const from = line === start.line ? start.col : 0;
            const to = line === end.line ? end.col + 1 : cols;
            text += cells.slice(from, to).join("");
            if (line !== end.line && !row?.wrapped) text = `${text.trimEnd()}\n`;
        }
        return text.trimEnd();
    }

    async copySelection(): Promise<boolean> {
        const range = this.selectionRange();
        if (!range) return false;
        const text = await invoke<string>("pty_text", { id: this.pid, from: range[0], to: range[1] }).catch(() => this.getSelection());
        if (!text) return false;
        await copyText(text);
        return true;
    }

    async pasteClipboard(): Promise<boolean> {
        const text = await readClipboardText();
        if (!text) return false;
        this.send({ type: "paste", text });
        return true;
    }

    selectAll(): void {
        const header = this.grid.header;
        this.anchor = { line: this.grid.historyStart, col: 0 };
        this.head = { line: header.historyEnd + header.rows - 1, col: header.cols };
        this.schedulePaint();
    }

    async copyScrollback(): Promise<boolean> {
        const text = await invoke<string>("pty_text", { id: this.pid });
        if (!text) return false;
        await copyText(text);
        return true;
    }

    clear(): void {
        this.dropMatches();
        this.anchor = this.head = null;
        void invoke("pty_clear", { id: this.pid }).catch(() => {});
    }

    // Find

    async find(query: string, direction: "next" | "previous", options: TerminalSearchOptions, incremental: boolean): Promise<boolean> {
        if (!query) {
            this.clearSearch();
            return false;
        }
        const current = this.activeMatch === null ? null : this.matches[this.activeMatch]?.[0];
        const from = current ?? { line: this.viewTop, col: 0 };
        let result: FindResult;
        try {
            result = await invoke<FindResult>("pty_find", {
                id: this.pid,
                query,
                options,
                from,
                backwards: direction === "previous",
                inclusive: incremental || current === undefined || current === null,
            });
        } catch {
            this.dropMatches();
            this.options.onSearchResults({ resultIndex: -1, resultCount: 0 });
            return false;
        }
        if (this.disposed) return false;
        this.matches = result.matches;
        this.activeMatch = result.current;
        this.searchEpoch = result.epoch;
        this.options.onSearchResults({ resultIndex: result.current ?? -1, resultCount: result.matches.length });
        const active = result.current === null ? null : result.matches[result.current];
        if (active) this.reveal(active[0].line);
        this.schedulePaint();
        return active !== null;
    }

    private reveal(line: number): void {
        const header = this.grid.header;
        if (line >= this.viewTop && line < this.viewTop + header.rows) return;
        const top = Math.max(this.grid.historyStart, line - Math.floor(header.rows / 2));
        this.top = top >= header.historyEnd ? null : top;
    }

    private dropMatches(): void {
        this.matches = [];
        this.activeMatch = null;
    }

    clearSearch(): void {
        this.dropMatches();
        this.options.onSearchResults({ resultIndex: -1, resultCount: 0 });
        this.schedulePaint();
    }

    dispose(): void {
        this.disposed = true;
        if (this.frameRequest !== null) cancelAnimationFrame(this.frameRequest);
        if (this.blinkTimer !== null) window.clearInterval(this.blinkTimer);
        if (this.ackTimer !== null) window.clearTimeout(this.ackTimer);
        for (const cleanup of this.cleanups.splice(0)) cleanup();
        this.canvas.dispose();
        this.element.remove();
    }
}
