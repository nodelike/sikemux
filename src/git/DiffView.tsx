import {
    memo,
    useEffect,
    useLayoutEffect,
    useMemo,
    useRef,
    useState,
    useSyncExternalStore,
    type CSSProperties,
    type KeyboardEvent,
    type RefObject,
} from "react";
import { DIFF_ROW, type DiffRow } from "../api/git";
import { CodeRun } from "../chat/codeHighlight";
import type { CodeLine } from "@shared/chat/types";
import { grammarFor } from "../languages";
import { currentTheme, subscribeTheme } from "../themes/bus";
import { codeThemeName } from "../themes/codeTheme";
import { colourLines, finishedColours, type Colours } from "./diffColours";

export const DIFF_TOKENIZE_MAX_LINES = 10_000;
const TOKENIZE_MAX_LINE_LENGTH = 1000;
const LINE_HEIGHT = 19;
const HIDDEN_HEIGHT = 4;
const PADDING_BLOCK = 8;
const TAB_SIZE = 2;
/* Rows are drawn in blocks of this many, each placed where it belongs, so
   scrolling adds or drops a whole block now and then and never moves the rows
   already drawn. */
const BLOCK_ROWS = 64;
const OVERSCAN_PX = 600;

const KIND = ["context", "added", "deleted", "hidden"] as const;

function rowHeight(row: DiffRow): number {
    return row[0] === DIFF_ROW.hidden ? HIDDEN_HEIGHT : LINE_HEIGHT;
}

/** Where each row starts, and the height of them all, below the top padding. */
export function layoutRows(rows: readonly DiffRow[]): { offsets: Float64Array; total: number; columns: number; digits: number } {
    const offsets = new Float64Array(rows.length + 1);
    let columns = 0;
    let widest = 0;
    for (let index = 0; index < rows.length; index++) {
        const row = rows[index];
        offsets[index + 1] = offsets[index] + rowHeight(row);
        if (row[0] === DIFF_ROW.hidden) continue;
        widest = Math.max(widest, row[1]);
        columns = Math.max(columns, displayWidth(row[2]));
    }
    return { offsets, total: offsets[rows.length], columns, digits: Math.max(3, String(widest).length) };
}

function displayWidth(text: string): number {
    let width = 0;
    for (let at = 0; at < text.length; at++) width += text.charCodeAt(at) === 9 ? TAB_SIZE - (width % TAB_SIZE) : 1;
    return width;
}

function firstRowBelow(offsets: Float64Array, y: number): number {
    let low = 0;
    let high = offsets.length - 1;
    while (low < high) {
        const mid = (low + high) >> 1;
        if (offsets[mid + 1] <= y) low = mid + 1;
        else high = mid;
    }
    return low;
}

function scrollParent(node: HTMLElement): HTMLElement | null {
    for (let at = node.parentElement; at; at = at.parentElement) {
        const { overflowY } = getComputedStyle(at);
        if (overflowY === "auto" || overflowY === "scroll") return at;
    }
    return null;
}

/** The blocks near the viewport of whatever scrolls the diff, which is usually a list of diffs rather than the diff itself. */
function useVisibleBlocks(host: RefObject<HTMLDivElement | null>, offsets: Float64Array): [number, number] {
    const count = offsets.length - 1;
    const blocks = Math.ceil(count / BLOCK_ROWS);
    const [range, setRange] = useState<[number, number]>(() => [0, Math.min(blocks, 1)]);

    useLayoutEffect(() => {
        const node = host.current;
        if (!node) return;
        const scroller = scrollParent(node);
        const measure = () => {
            const box = node.getBoundingClientRect();
            const view = scroller ? scroller.getBoundingClientRect() : { top: 0, bottom: window.innerHeight };
            const top = view.top - box.top - PADDING_BLOCK - OVERSCAN_PX;
            const bottom = view.bottom - box.top - PADDING_BLOCK + OVERSCAN_PX;
            const first = Math.floor(firstRowBelow(offsets, Math.max(0, top)) / BLOCK_ROWS);
            const last = Math.floor(Math.min(count - 1, firstRowBelow(offsets, Math.max(0, bottom))) / BLOCK_ROWS);
            const next: [number, number] = bottom < 0 || top >= offsets[count] ? [0, 0] : [first, Math.min(blocks, last + 1)];
            setRange((current) => (current[0] === next[0] && current[1] === next[1] ? current : next));
        };
        measure();
        const target: HTMLElement | Window = scroller ?? window;
        target.addEventListener("scroll", measure, { passive: true });
        const resize = new ResizeObserver(measure);
        resize.observe(node);
        if (scroller) resize.observe(scroller);
        return () => {
            target.removeEventListener("scroll", measure);
            resize.disconnect();
        };
    }, [host, offsets, count, blocks]);

    return range;
}

/** The block a selection end sits in; an end outside the diff counts as its first or last block. */
function blockOf(end: Node | null, host: HTMLElement, blocks: number): number {
    const row = (end instanceof Element ? end : end?.parentElement)?.closest<HTMLElement>("[data-row]");
    if (row && host.contains(row)) return Math.floor(Number(row.dataset.row) / BLOCK_ROWS);
    return end && host.compareDocumentPosition(end) & Node.DOCUMENT_POSITION_FOLLOWING ? blocks - 1 : 0;
}

/**
 * The blocks to draw: those near the viewport, and every block a selection
 * runs across, so dragging a selection down a long diff keeps hold of where
 * it started.
 */
function useDrawnBlocks(host: RefObject<HTMLDivElement | null>, blocks: number, [first, end]: [number, number]): readonly number[] {
    const [selected, setSelected] = useState<[number, number] | null>(null);

    useEffect(() => {
        const read = () => {
            const node = host.current;
            const selection = document.getSelection();
            let next: [number, number] | null = null;
            if (node && selection && !selection.isCollapsed && selection.containsNode(node, true)) {
                const from = blockOf(selection.anchorNode, node, blocks);
                const to = blockOf(selection.focusNode, node, blocks);
                next = [Math.min(from, to), Math.max(from, to) + 1];
            }
            setSelected((current) => (current?.[0] === next?.[0] && current?.[1] === next?.[1] ? current : next));
        };
        document.addEventListener("selectionchange", read);
        return () => document.removeEventListener("selectionchange", read);
    }, [host, blocks]);

    return useMemo(() => {
        const drawn = new Set<number>();
        for (let block = first; block < end; block++) drawn.add(block);
        if (selected) for (let block = selected[0]; block < selected[1]; block++) drawn.add(block);
        return [...drawn].sort((a, b) => a - b);
    }, [first, end, selected]);
}

const themeNameNow = () => codeThemeName(currentTheme());

/** The lines each side of a diff had, so each is coloured as the file it came from. */
function sidesOf(rows: readonly DiffRow[]): { before: string[]; after: string[]; deletes: boolean } {
    const before: string[] = [];
    const after: string[] = [];
    let deletes = false;
    for (const [kind, , text] of rows) {
        if (kind === DIFF_ROW.hidden) continue;
        if (kind !== DIFF_ROW.added) before.push(text);
        if (kind !== DIFF_ROW.deleted) after.push(text);
        if (kind === DIFF_ROW.deleted) deletes = true;
    }
    return { before, after, deletes };
}

function useColours(lines: readonly string[] | null, lang: string | null, themeName: string): Colours | null {
    const [arrived, setArrived] = useState<{ lines: readonly string[]; colours: Colours } | null>(null);
    const ready = useMemo(() => (lines && lang ? finishedColours(lines, lang, themeName) : undefined), [lines, lang, themeName]);

    useEffect(() => {
        if (!lines || !lang || ready) return;
        return colourLines(lines, lang, currentTheme(), themeName, TOKENIZE_MAX_LINE_LENGTH, (colours) => setArrived({ lines, colours }));
    }, [lines, lang, themeName, ready]);

    if (!lines || !lang) return null;
    return ready ?? (arrived?.lines === lines ? arrived.colours : null);
}

/**
 * Colours for each row. The two sides are read as the files they came from,
 * so a deleted line is coloured by the file it left and an unchanged line by
 * the file it is in now. Rows stay plain until their colours arrive.
 */
function useRowTokens(rows: readonly DiffRow[], path: string): readonly (CodeLine | undefined)[] | null {
    const themeName = useSyncExternalStore(subscribeTheme, themeNameNow, themeNameNow);
    const lang = grammarFor(path);
    const sides = useMemo(() => {
        const split = sidesOf(rows);
        return split.before.length > DIFF_TOKENIZE_MAX_LINES || split.after.length > DIFF_TOKENIZE_MAX_LINES ? null : split;
    }, [rows]);
    const newSide = useColours(sides?.after ?? null, lang, themeName);
    const oldSide = useColours(sides?.deletes ? sides.before : null, lang, themeName);

    return useMemo(() => {
        if (!newSide && !oldSide) return null;
        const lines: (CodeLine | undefined)[] = [];
        let old = 0;
        let current = 0;
        for (const [kind] of rows) {
            if (kind === DIFF_ROW.deleted) lines.push(oldSide?.[old]);
            else if (kind === DIFF_ROW.hidden) lines.push(undefined);
            else lines.push(newSide?.[current]);
            if (kind === DIFF_ROW.context || kind === DIFF_ROW.deleted) old++;
            if (kind === DIFF_ROW.context || kind === DIFF_ROW.added) current++;
        }
        return lines;
    }, [rows, newSide, oldSide]);
}

export const DiffView = memo(function DiffView({
    rows,
    path,
    tinted,
    notes,
    onShowHidden,
}: {
    rows: readonly DiffRow[];
    path: string;
    tinted: boolean;
    /** A quiet note after each row's code, such as the agent that wrote it. */
    notes?: readonly (string | undefined)[] | null;
    onShowHidden?: () => void;
}) {
    const host = useRef<HTMLDivElement>(null);
    const layout = useMemo(() => layoutRows(rows), [rows]);
    const drawn = useDrawnBlocks(host, Math.ceil(rows.length / BLOCK_ROWS), useVisibleBlocks(host, layout.offsets));
    const tokens = useRowTokens(rows, path);
    const noteColumns = useMemo(() => {
        let widest = 0;
        notes?.forEach((note, index) => {
            if (note) widest = Math.max(widest, displayWidth(rows[index]?.[2] ?? "") + note.length + 4);
        });
        return widest;
    }, [rows, notes]);

    if (rows.length === 0) return null;

    const style = { "--diff-digits": layout.digits } as CSSProperties;
    // Blocks are placed rather than stacked, so the rows cannot widen the list; it is as wide as its longest line says.
    const columns = Math.max(layout.columns, noteColumns);
    const width = `max(100%, calc(${layout.digits + columns + 5}ch + 2px))`;

    return (
        <div ref={host} className={`diff-view${tinted ? " tinted" : ""}`} style={style}>
            <div className="diff-rows" style={{ minWidth: width, height: layout.total }}>
                {drawn.map((block) => (
                    <DiffBlock
                        key={block}
                        rows={rows}
                        offsets={layout.offsets}
                        start={block * BLOCK_ROWS}
                        end={Math.min(rows.length, (block + 1) * BLOCK_ROWS)}
                        tokens={tokens}
                        notes={notes}
                        onShowHidden={onShowHidden}
                    />
                ))}
            </div>
        </div>
    );
});

const DiffBlock = memo(function DiffBlock({
    rows,
    offsets,
    start,
    end,
    tokens,
    notes,
    onShowHidden,
}: {
    rows: readonly DiffRow[];
    offsets: Float64Array;
    start: number;
    end: number;
    tokens: readonly (CodeLine | undefined)[] | null;
    notes?: readonly (string | undefined)[] | null;
    onShowHidden?: () => void;
}) {
    const showHidden = (event: KeyboardEvent) => {
        if (event.key !== "Enter" && event.key !== " ") return;
        event.preventDefault();
        onShowHidden?.();
    };
    const lines = [];
    for (let index = start; index < end; index++) {
        const row = rows[index];
        if (row[0] === DIFF_ROW.hidden) {
            const label = `Show ${row[1]} unchanged ${row[1] === 1 ? "line" : "lines"}`;
            lines.push(
                <div
                    key={index}
                    className="diff-row"
                    data-kind="hidden"
                    role="button"
                    tabIndex={0}
                    aria-label={label}
                    title={label}
                    onClick={onShowHidden}
                    onKeyDown={showHidden}
                />,
            );
        } else lines.push(<DiffLine key={index} index={index} row={row} tokens={tokens?.[index]} note={notes?.[index]} />);
    }
    return (
        <div className="diff-block" style={{ top: offsets[start], height: offsets[end] - offsets[start] }}>
            {lines}
        </div>
    );
});

const DiffLine = memo(function DiffLine({
    index,
    row,
    tokens,
    note,
}: {
    index: number;
    row: DiffRow;
    tokens: CodeLine | undefined;
    note: string | undefined;
}) {
    return (
        <div className="diff-row" data-kind={KIND[row[0]]} data-row={index}>
            <span className="diff-num">{row[1]}</span>
            <span className="diff-code">
                {tokens ? <CodeRun tokens={tokens} /> : row[2]}
                {note && <span className="diff-note">{note}</span>}
            </span>
        </div>
    );
});
