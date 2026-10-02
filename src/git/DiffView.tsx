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
import type { CodeLine } from "../chat/types";
import { grammarFor } from "../languages";
import { swallow } from "../state/toast";
import { currentTheme, subscribeTheme } from "../themes/bus";
import { codeThemeName } from "../themes/codeTheme";

export const DIFF_TOKENIZE_MAX_LINES = 4000;
const TOKENIZE_MAX_LINE_LENGTH = 1000;
const LINE_HEIGHT = 19;
const HIDDEN_HEIGHT = 4;
const PADDING_BLOCK = 8;
const TAB_SIZE = 2;
/* Up to this many rows are all in the document, so a selection can run the
   whole diff. Past it only the rows near the viewport are. */
const RENDER_ALL_ROWS = 1500;
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

/** The rows near the viewport of whatever scrolls the diff, which is usually a list of diffs rather than the diff itself. */
function useVisibleRows(host: RefObject<HTMLDivElement | null>, offsets: Float64Array, enabled: boolean): [number, number] {
    const count = offsets.length - 1;
    const [range, setRange] = useState<[number, number]>(() => [0, enabled ? Math.min(count, 60) : count]);

    useLayoutEffect(() => {
        const node = host.current;
        if (!enabled || !node) {
            setRange([0, count]);
            return;
        }
        const scroller = scrollParent(node);
        const measure = () => {
            const box = node.getBoundingClientRect();
            const view = scroller ? scroller.getBoundingClientRect() : { top: 0, bottom: window.innerHeight };
            const top = view.top - box.top - PADDING_BLOCK - OVERSCAN_PX;
            const bottom = view.bottom - box.top - PADDING_BLOCK + OVERSCAN_PX;
            const next: [number, number] = [
                firstRowBelow(offsets, Math.max(0, top)),
                Math.min(count, firstRowBelow(offsets, Math.max(0, bottom)) + 1),
            ];
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
    }, [host, offsets, count, enabled]);

    return enabled ? range : [0, count];
}

const themeNameNow = () => codeThemeName(currentTheme());

/**
 * Colours for each row. The two sides are read as the files they came from,
 * so a deleted line is coloured by the file it left and an unchanged line by
 * the file it is in now. Rows stay plain until the colours arrive.
 */
function useRowTokens(rows: readonly DiffRow[], path: string): readonly (CodeLine | undefined)[] | null {
    const themeName = useSyncExternalStore(subscribeTheme, themeNameNow, themeNameNow);
    const lang = grammarFor(path);
    const [tokens, setTokens] = useState<{ rows: readonly DiffRow[]; lines: (CodeLine | undefined)[] } | null>(null);

    useEffect(() => {
        const before: string[] = [];
        const after: string[] = [];
        let deletes = false;
        for (const [kind, , text] of rows) {
            if (kind === DIFF_ROW.hidden) continue;
            if (kind !== DIFF_ROW.added) before.push(text);
            if (kind !== DIFF_ROW.deleted) after.push(text);
            if (kind === DIFF_ROW.deleted) deletes = true;
        }
        if (!lang || before.length > DIFF_TOKENIZE_MAX_LINES || after.length > DIFF_TOKENIZE_MAX_LINES) return;
        let stale = false;
        const options = { maxLineLength: TOKENIZE_MAX_LINE_LENGTH, stale: () => stale };
        const theme = currentTheme();
        void import("../chat/shikiTokens")
            .then((shiki) =>
                Promise.all([
                    shiki.tokenizeLines(after, lang, theme, themeName, options),
                    deletes ? shiki.tokenizeLines(before, lang, theme, themeName, options) : null,
                ]),
            )
            .then(([newSide, oldSide]) => {
                if (stale || !newSide) return;
                const lines: (CodeLine | undefined)[] = [];
                let old = 0;
                let current = 0;
                for (const [kind] of rows) {
                    if (kind === DIFF_ROW.deleted) lines.push(oldSide?.[old]);
                    else if (kind === DIFF_ROW.hidden) lines.push(undefined);
                    else lines.push(newSide[current]);
                    if (kind === DIFF_ROW.context || kind === DIFF_ROW.deleted) old++;
                    if (kind === DIFF_ROW.context || kind === DIFF_ROW.added) current++;
                }
                setTokens({ rows, lines });
            })
            .catch(swallow("colour a diff"));
        return () => {
            stale = true;
        };
    }, [rows, lang, themeName]);

    return tokens?.rows === rows ? tokens.lines : null;
}

export function DiffView({
    rows,
    path,
    tinted,
    onShowHidden,
}: {
    rows: readonly DiffRow[];
    path: string;
    tinted: boolean;
    onShowHidden?: () => void;
}) {
    const host = useRef<HTMLDivElement>(null);
    const layout = useMemo(() => layoutRows(rows), [rows]);
    const windowed = rows.length > RENDER_ALL_ROWS;
    const [start, end] = useVisibleRows(host, layout.offsets, windowed);
    const tokens = useRowTokens(rows, path);

    if (rows.length === 0) return null;

    const style = { "--diff-digits": layout.digits } as CSSProperties;
    // Rows off screen are not there to widen the list, so a windowed diff is as wide as its longest line says it should be.
    const width = windowed ? `max(100%, calc(${layout.digits + layout.columns + 5}ch + 2px))` : undefined;
    const showHidden = (event: KeyboardEvent) => {
        if (event.key !== "Enter" && event.key !== " ") return;
        event.preventDefault();
        onShowHidden?.();
    };

    return (
        <div ref={host} className={`diff-view${tinted ? " tinted" : ""}`} style={style}>
            <div
                className="diff-rows"
                style={{ minWidth: width, paddingTop: layout.offsets[start], paddingBottom: layout.total - layout.offsets[end] }}>
                {rows.slice(start, end).map((row, offset) => {
                    const index = start + offset;
                    if (row[0] === DIFF_ROW.hidden) {
                        const label = `Show ${row[1]} unchanged ${row[1] === 1 ? "line" : "lines"}`;
                        return (
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
                            />
                        );
                    }
                    return <DiffLine key={index} index={index} row={row} tokens={tokens?.[index]} />;
                })}
            </div>
        </div>
    );
}

const DiffLine = memo(function DiffLine({ index, row, tokens }: { index: number; row: DiffRow; tokens: CodeLine | undefined }) {
    return (
        <div className="diff-row" data-kind={KIND[row[0]]} data-row={index}>
            <span className="diff-num">{row[1]}</span>
            <span className="diff-code">{tokens ? <CodeRun tokens={tokens} /> : row[2]}</span>
        </div>
    );
});
