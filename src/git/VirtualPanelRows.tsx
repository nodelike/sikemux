import { Fragment, useEffect, useRef, type ReactNode } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { PANEL_ROW_HEIGHT } from "../ui/Panel";

const VIRTUAL_THRESHOLD = 100;

export function VirtualPanelRows<T>({
    items,
    selectedIndex,
    focused,
    estimateSize = PANEL_ROW_HEIGHT,
    getKey,
    renderRow,
}: {
    items: T[];
    selectedIndex: number;
    focused: boolean;
    estimateSize?: number;
    getKey: (item: T, index: number) => string;
    renderRow: (item: T, index: number) => ReactNode;
}) {
    const rootRef = useRef<HTMLDivElement>(null);
    const virtual = items.length > VIRTUAL_THRESHOLD;
    const virtualizer = useVirtualizer({
        count: virtual ? items.length : 0,
        getScrollElement: () => rootRef.current?.parentElement ?? null,
        estimateSize: () => estimateSize,
        overscan: 12,
        getItemKey: (index) => getKey(items[index], index),
    });

    useEffect(() => {
        if (!virtual || !focused || selectedIndex < 0 || selectedIndex >= items.length) return;
        virtualizer.scrollToIndex(selectedIndex, { align: "auto" });
    }, [focused, items.length, selectedIndex, virtual, virtualizer]);

    if (!virtual) {
        return items.map((item, index) => <Fragment key={getKey(item, index)}>{renderRow(item, index)}</Fragment>);
    }

    return (
        <div ref={rootRef} className="git-virtual-rows" style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
            {virtualizer.getVirtualItems().map((row) => {
                const item = items[row.index];
                return (
                    <div
                        key={row.key}
                        style={{
                            position: "absolute",
                            top: 0,
                            left: 0,
                            right: 0,
                            height: row.size,
                            transform: `translateY(${row.start}px)`,
                        }}>
                        {renderRow(item, row.index)}
                    </div>
                );
            })}
        </div>
    );
}
