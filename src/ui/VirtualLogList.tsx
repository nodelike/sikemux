import { useEffect, useRef, type CSSProperties, type ReactNode } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";

interface VirtualLogListProps<T> {
    items: T[];
    className: string;
    rowClassName: string | ((item: T, index: number) => string);
    estimateSize?: number;
    overscan?: number;
    follow?: boolean;
    empty?: ReactNode;
    onScroll?: (element: HTMLDivElement) => void;
    allowFollow?: (element: HTMLDivElement) => boolean;
    getItemKey?: (item: T, index: number) => string | number;
    /** Scrolls to a row each time a new object is passed, so the same row can be asked for twice. */
    jumpTo?: { index: number } | null;
    renderRow: (item: T, index: number) => ReactNode;
}

export function VirtualLogList<T>({
    items,
    className,
    rowClassName,
    estimateSize = 20,
    overscan = 24,
    follow = false,
    empty,
    onScroll,
    allowFollow,
    getItemKey,
    jumpTo,
    renderRow,
}: VirtualLogListProps<T>) {
    const scrollRef = useRef<HTMLDivElement>(null);
    const virtualizer = useVirtualizer({
        count: items.length,
        getScrollElement: () => scrollRef.current,
        estimateSize: () => estimateSize,
        overscan,
        getItemKey: (index) => getItemKey?.(items[index], index) ?? index,
        measureElement: (element) => element.getBoundingClientRect().height,
    });

    useEffect(() => {
        if (!follow || items.length === 0) return;
        const element = scrollRef.current;
        if (element && allowFollow?.(element) === false) return;
        virtualizer.scrollToIndex(items.length - 1, { align: "end" });
    }, [allowFollow, follow, items.length, virtualizer]);

    const count = useRef(items.length);
    count.current = items.length;
    useEffect(() => {
        if (!jumpTo || jumpTo.index < 0 || jumpTo.index >= count.current) return;
        virtualizer.scrollToIndex(jumpTo.index, { align: "start" });
    }, [jumpTo, virtualizer]);

    const virtualItems = virtualizer.getVirtualItems();

    return (
        <div
            className={className}
            ref={scrollRef}
            onScroll={() => {
                const element = scrollRef.current;
                if (element) onScroll?.(element);
            }}>
            {items.length === 0 && empty}
            {items.length > 0 && (
                <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
                    {virtualItems.map((vi) => {
                        const item = items[vi.index];
                        if (item == null) return null;
                        const style: CSSProperties = {
                            position: "absolute",
                            top: 0,
                            left: 0,
                            right: 0,
                            transform: `translateY(${vi.start}px)`,
                        };
                        const className = typeof rowClassName === "function" ? rowClassName(item, vi.index) : rowClassName;
                        return (
                            <div key={vi.key} ref={virtualizer.measureElement} data-index={vi.index} className={className} style={style}>
                                {renderRow(item, vi.index)}
                            </div>
                        );
                    })}
                </div>
            )}
        </div>
    );
}
