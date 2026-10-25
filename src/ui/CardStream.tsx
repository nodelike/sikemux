import { useEffect, useImperativeHandle, useRef, type CSSProperties, type ReactNode, type Ref } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { IconChevron } from "@shared/ui/Icons";
import { Tooltip } from "./Tooltip";

export const CARD_HEADER_HEIGHT = 29;

export interface CardStreamHandle {
    /** Brings a card's header to the top of the list. */
    scrollToCard: (index: number) => void;
    /** Where a card begins in the list's scrolled content, by its height so far. */
    cardStart: (index: number) => number;
    scroller: () => HTMLDivElement | null;
}

export interface CardStreamProps {
    keys: readonly string[];
    collapsed: ReadonlySet<string>;
    onToggle: (key: string) => void;
    /** How tall a card is likely to be while open, before it is measured. */
    estimate: (key: string) => number;
    toggleLabel: (key: string, open: boolean) => string;
    /** What a card's header shows after its chevron. */
    header: (key: string, open: boolean) => ReactNode;
    /** What an open card shows under its header; `start` is where the card begins in the list's scrolled content. */
    body: (key: string, start: number) => ReactNode;
    itemClassName?: (key: string) => string;
    handle?: Ref<CardStreamHandle>;
    onScroll?: (list: HTMLDivElement) => void;
}

/**
 * Collapsible cards in one virtualized stream, each header pinned while its body scrolls under it. The git pane's
 * review files and a CI job's steps are both read through it.
 */
export function CardStream({ keys, collapsed, onToggle, estimate, toggleLabel, header, body, itemClassName, handle, onScroll }: CardStreamProps) {
    const itemRefs = useRef(new Map<string, HTMLDivElement>());
    const listRef = useRef<HTMLDivElement>(null);
    const virtualizer = useVirtualizer({
        count: keys.length,
        getScrollElement: () => listRef.current,
        estimateSize: (index) => {
            const key = keys[index];
            if (key === undefined || collapsed.has(key)) return CARD_HEADER_HEIGHT;
            return estimate(key);
        },
        // A card folded and the same card open are told apart, so either is measured afresh rather than keeping the other's height.
        getItemKey: (index) => {
            const key = keys[index];
            return key === undefined ? index : `${collapsed.has(key) ? "folded" : "open"}:${key}`;
        },
        overscan: 2,
        initialRect: { width: 1000, height: 800 },
    });

    useImperativeHandle(
        handle,
        () => ({
            scrollToCard: (index: number) => virtualizer.scrollToIndex(index, { align: "start" }),
            cardStart: (index: number) => virtualizer.getOffsetForIndex(index, "start")?.[0] ?? 0,
            scroller: () => listRef.current,
        }),
        [virtualizer],
    );

    // A pinned header's rounded corners would show the body scrolled under it, so each open card's body is trimmed at
    // the header's bottom while the list scrolls.
    useEffect(() => {
        const list = listRef.current;
        if (!list) return;
        let frame = 0;
        const trim = () => {
            frame = 0;
            const top = list.scrollTop;
            for (const row of virtualizer.getVirtualItems()) {
                const key = keys[row.index];
                const item = key === undefined ? undefined : itemRefs.current.get(key);
                item?.style.setProperty("--pinned-cut", `${Math.max(0, top - row.start)}px`);
            }
        };
        const scrolled = () => {
            if (!frame) frame = window.requestAnimationFrame(trim);
        };
        trim();
        list.addEventListener("scroll", scrolled, { passive: true });
        return () => {
            list.removeEventListener("scroll", scrolled);
            window.cancelAnimationFrame(frame);
        };
    }, [virtualizer, keys]);

    return (
        <div className="merge-review-list" ref={listRef} onScroll={onScroll ? (event) => onScroll(event.currentTarget) : undefined}>
            <div className="merge-review-virtual" style={{ height: virtualizer.getTotalSize() }}>
                {virtualizer.getVirtualItems().map((row) => {
                    const key = keys[row.index];
                    if (key === undefined) return null;
                    const open = !collapsed.has(key);
                    const extra = itemClassName?.(key);
                    // Placed by `top`, not a transform: WebKit draws a sticky header inside a transformed box at the box's bottom and leaves it there as the list scrolls.
                    const style: CSSProperties = { top: row.start, height: row.size, overflow: "clip" };
                    return (
                        <div key={row.key} className="merge-review-virtual-item" style={style}>
                            <div ref={virtualizer.measureElement} data-index={row.index}>
                                <div
                                    className={`acc-item merge-review-item${open ? " open" : ""}${extra ? ` ${extra}` : ""}`}
                                    ref={(node) => {
                                        if (node) itemRefs.current.set(key, node);
                                        else itemRefs.current.delete(key);
                                    }}>
                                    <div className="acc-header merge-file-header" onClick={() => onToggle(key)}>
                                        <Tooltip label={open ? "Collapse" : "Expand"}>
                                            <button type="button" className="acc-toggle" aria-label={toggleLabel(key, open)}>
                                                <span className={`acc-chev${open ? " open" : ""}`}>
                                                    <IconChevron size={11} />
                                                </span>
                                            </button>
                                        </Tooltip>
                                        {header(key, open)}
                                    </div>
                                    {open && body(key, row.start)}
                                </div>
                            </div>
                        </div>
                    );
                })}
            </div>
        </div>
    );
}
