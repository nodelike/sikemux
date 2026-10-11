import { useCallback, useLayoutEffect, useRef, useState, type RefObject } from "react";

// How far above the last line still counts as reading the latest message.
export const BOTTOM_SLACK = 72;

/* How far the reader is from the bottom. The transcript is laid out from the
   bottom, so its scroll position is 0 there and negative above it. */
const distanceFromBottom = (scroller: HTMLElement) => Math.max(0, -scroller.scrollTop);

/*
 * The transcript is laid out from the bottom up, so the browser itself holds
 * the bottom still as a reply grows, and holds the reader's place as anything
 * above them changes height. Nothing here moves the scroll position while the
 * reader is scrolling.
 *
 * The one change the browser cannot absorb is something growing below the
 * reader, such as a reply streaming in while they read further up. That would
 * push what they are reading up the screen, so the row at the top of the view
 * is noted on every scroll and put back where it was. It is held by its bottom
 * edge: laid out from the bottom, a row that grows grows upwards, and only
 * growth below it moves that edge. The last row is the exception, held by its
 * top edge, since it grows as the reply streams into it while it is read.
 */
export function useStickToBottom({
    scrollRef,
    contentRef,
    visible,
}: {
    scrollRef: RefObject<HTMLDivElement | null>;
    contentRef: RefObject<HTMLDivElement | null>;
    visible: boolean;
}) {
    const [atBottom, setAtBottom] = useState(true);
    const atBottomRef = useRef(true);
    const anchorRef = useRef<{ row: Element; edge: "top" | "bottom"; offset: number } | null>(null);

    const noteAnchor = useCallback(() => {
        const scroller = scrollRef.current;
        anchorRef.current = null;
        if (!scroller || atBottomRef.current) return;
        const view = scroller.getBoundingClientRect();
        for (const offset of [1, 24, 64]) {
            const row = document.elementFromPoint?.(view.left + view.width / 2, view.top + offset)?.closest(".chat-row");
            if (row && scroller.contains(row)) {
                const edge = Array.from(scroller.querySelectorAll(".chat-row")).at(-1) === row ? "top" : "bottom";
                anchorRef.current = { row, edge, offset: row.getBoundingClientRect()[edge] - view.top };
                return;
            }
        }
    }, [scrollRef]);

    useLayoutEffect(() => {
        const content = contentRef.current;
        const scroller = scrollRef.current;
        if (!content || !scroller || typeof ResizeObserver === "undefined") return;
        const resized = new ResizeObserver(() => {
            const anchor = anchorRef.current;
            if (atBottomRef.current || !anchor?.row.isConnected) return;
            const moved = anchor.row.getBoundingClientRect()[anchor.edge] - scroller.getBoundingClientRect().top - anchor.offset;
            if (Math.abs(moved) >= 1) scroller.scrollTop += moved;
        });
        resized.observe(content);
        return () => resized.disconnect();
    }, [contentRef, scrollRef]);

    /* A pane coming back into view keeps the place it was left at, unless that
       was the bottom, which may have moved while it was hidden. */
    useLayoutEffect(() => {
        const scroller = scrollRef.current;
        if (visible && scroller && atBottomRef.current) scroller.scrollTop = 0;
    }, [visible, scrollRef]);

    const settle = useCallback((next: boolean) => {
        if (next === atBottomRef.current) return;
        atBottomRef.current = next;
        setAtBottom(next);
    }, []);

    const onScroll = () => {
        const scroller = scrollRef.current;
        if (!scroller) return;
        settle(distanceFromBottom(scroller) < BOTTOM_SLACK);
        noteAnchor();
    };

    const jumpToBottom = () => {
        const scroller = scrollRef.current;
        settle(true);
        anchorRef.current = null;
        if (scroller) scroller.scrollTop = 0;
    };

    return { atBottom, onScroll, jumpToBottom };
}
