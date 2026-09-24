import { createContext, useContext, useMemo, useState } from "react";

const LONG_TEXT_LIMIT = 20_000;

export interface TextCut {
    head: string;
    hidden: number;
}

function openFence(text: string): string | null {
    let open: string | null = null;
    for (const [, fence] of text.matchAll(/^ {0,3}(`{3,}|~{3,})/gm)) {
        if (open === null) open = fence;
        else if (fence[0] === open[0] && fence.length >= open.length) open = null;
    }
    return open;
}

/** Prefers a paragraph break, then a line break, and cuts mid-line only when neither is near the limit. */
export function cutLongText(text: string, limit = LONG_TEXT_LIMIT): TextCut | null {
    if (text.length <= limit) return null;

    const floor = Math.floor(limit / 2);
    let at = text.lastIndexOf("\n\n", limit);
    if (at < floor) at = text.lastIndexOf("\n", limit);
    if (at < floor) at = limit;

    const hidden = text.length - at;
    if (hidden < limit / 10) return null;

    let head = text.slice(0, at);
    // Left open, the fence would turn everything after the cut into code.
    const fence = openFence(head);
    if (fence) head += `\n${fence}`;
    return { head, hidden };
}

/* Rows unmount when they scroll out of view, so what the reader has seen of each
   long message is kept by the pane instead. */
export type FoldMemory = { streamed: Set<string>; expanded: Set<string> };
export const newFoldMemory = (): FoldMemory => ({ streamed: new Set(), expanded: new Set() });
export const FoldMemoryContext = createContext<FoldMemory>(newFoldMemory());

// A message the reader watched stream in is never folded.
export function useLongTextFold(id: string, text: string, live: boolean) {
    const memory = useContext(FoldMemoryContext);
    const [expanded, setExpanded] = useState(() => memory.expanded.has(id));
    if (live) memory.streamed.add(id);
    const folds = !expanded && !memory.streamed.has(id);
    const cut = useMemo(() => (folds ? cutLongText(text) : null), [folds, text]);
    const expand = () => {
        memory.expanded.add(id);
        setExpanded(true);
    };
    return { cut, expand };
}
