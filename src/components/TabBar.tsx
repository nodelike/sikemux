import { useVirtualizer } from "@tanstack/react-virtual";
import { useLayoutEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import { prefersReducedMotion } from "../lib/motion";
import { TreeContextMenu, type CtxItem } from "./FileTree";
import { IconClose } from "./Icons";
import { Tooltip } from "./Tooltip";
import { useTabReorder, type TabDropRule, type TabReorderHandler } from "./useTabReorder";

/**
 * One normalized tab. Every tab strip in the app (editor files, agents,
 * terminals, plugin documents) describes its tabs as these, so selection,
 * closing, the dirty dot, accessories and the right-click menu all behave
 * identically. A new group only has to map its state into `TabDescriptor[]`.
 */
export interface TabDescriptor {
    /** Stable identity — used as the React key and passed to onSelect/onClose. */
    id: string;
    label: string;
    tabId?: string;
    panelId?: string;
    /** Leading glyph/badge rendered before the label (FileIcon, agent glyph, method badge…). */
    icon?: ReactNode;
    /** Show the unsaved-changes dot. */
    dirty?: boolean;
    active?: boolean;
    /** Defaults to whether `onClose` is provided; set false to pin a tab open. */
    closable?: boolean;
    title?: string;
    /** Per-tab status mark (spinner, activity dot). Takes the trailing slot, and
     * the close button takes it back under the pointer. */
    accessory?: ReactNode;
    /** Sits after the label, just before the close button, and stays visible. */
    badge?: ReactNode;
    className?: string;
}

export type TabVariant = "editor" | "agent" | "browser" | "stack";

/**
 * Brings a tab into view by scrolling the strip and only the strip.
 * `scrollIntoView` scrolls every scrollable ancestor as well, and whatever room
 * the strip runs out of it takes out of the stage the strip sits on, which
 * leaves the tabs and the window under them parked to one side for good.
 */
function reveal(strip: HTMLElement | null, tab: HTMLElement | undefined): void {
    if (!strip || !tab || !strip.scrollBy) return;
    const edge = strip.getBoundingClientRect();
    const box = tab.getBoundingClientRect();
    const off = box.left < edge.left ? box.left - edge.left : box.right > edge.right ? box.right - edge.right : 0;
    if (off !== 0) strip.scrollBy({ left: off, behavior: prefersReducedMotion() ? "auto" : "smooth" });
}

interface TabBarProps {
    variant: TabVariant;
    tabs: TabDescriptor[];
    onSelect: (id: string) => void;
    onClose?: (id: string) => void;
    /** Build the right-click menu for a tab. Omit to disable the context menu. */
    buildMenu?: (id: string) => CtxItem[];
    onAdd?: () => void;
    addIcon?: ReactNode;
    addTitle?: string;
    /** Spoken name for the add button, when the tooltip's wording reads badly aloud. */
    addLabel?: string;
    trailing?: ReactNode;
    /** Names the strip for assistive tech when more than one is on screen. */
    ariaLabel?: string;
    /** Enables press-and-drag reordering. Omit and the strip's order is fixed. */
    onReorder?: TabReorderHandler;
    /** Rules out drops the owner cannot honour, such as a file leaving its editor. */
    canReorder?: TabDropRule;
}

export function TabBar({
    variant,
    tabs,
    onSelect,
    onClose,
    buildMenu,
    onAdd,
    addIcon,
    addTitle,
    addLabel,
    trailing,
    ariaLabel,
    onReorder,
    canReorder,
}: TabBarProps) {
    const [menu, setMenu] = useState<{ x: number; y: number; id: string } | null>(null);
    const menuItems = menu && buildMenu ? buildMenu(menu.id) : null;
    const scrollRef = useRef<HTMLDivElement>(null);
    const tabRefs = useRef(new Map<string, HTMLButtonElement>());
    const virtualized = tabs.length > 40;
    const tabVirtualizer = useVirtualizer({
        horizontal: true,
        count: tabs.length,
        getScrollElement: () => scrollRef.current,
        estimateSize: () => 160,
        measureElement: (element) => element.getBoundingClientRect().width + 4,
        getItemKey: (index) => tabs[index]?.id ?? index,
        overscan: 8,
        enabled: virtualized,
    });
    const reorder = useTabReorder(
        tabRefs,
        tabs.map((tab) => tab.id),
        onReorder,
        canReorder,
    );
    const activeIndex = tabs.findIndex((tab) => tab.active);
    const activeId = tabs[activeIndex]?.id;

    useLayoutEffect(() => {
        if (virtualized && activeIndex >= 0) tabVirtualizer.scrollToIndex(activeIndex, { align: "auto" });
    }, [activeIndex, tabVirtualizer, virtualized]);

    useLayoutEffect(() => {
        if (activeId === undefined) return;
        // A virtualized strip may not have mounted the pill yet — the
        // virtualizer above has it roughly in view.
        reveal(scrollRef.current, tabRefs.current.get(activeId));
    }, [activeId]);

    const focusTabAt = (index: number) => {
        const tab = tabs[index];
        if (!tab) return;
        onSelect(tab.id);
        if (virtualized) tabVirtualizer.scrollToIndex(index, { align: "auto" });
        // Focusing brings the tab into view the same way `scrollIntoView` does,
        // ancestors and all, so the strip is left to reveal it on its own.
        const element = tabRefs.current.get(tab.id);
        if (element) element.focus({ preventScroll: true });
        else requestAnimationFrame(() => tabRefs.current.get(tab.id)?.focus({ preventScroll: true }));
    };

    const virtualItems = virtualized ? tabVirtualizer.getVirtualItems() : [];
    const firstVirtual = virtualItems[0];
    const lastVirtual = virtualItems.at(-1);
    const visibleTabs = virtualized
        ? virtualItems.map((item) => ({ tab: tabs[item.index], index: item.index }))
        : tabs.map((tab, index) => ({ tab, index }));

    return (
        <div ref={scrollRef} className={`tabbar v-${variant}${reorder.dragging ? " is-reordering" : ""}`} role="tablist" aria-label={ariaLabel}>
            {virtualized && <div aria-hidden="true" style={{ flex: `0 0 ${firstVirtual?.start ?? 0}px` }} />}
            {visibleTabs.map(({ tab: t, index }) => {
                const closable = t.closable ?? !!onClose;
                // One mark at a time: a tab that is busy says so, a tab that is
                // only unsaved shows the dot.
                const status = t.accessory ?? (t.dirty ? <span className="tab-dot" aria-hidden="true" /> : null);
                return (
                    <div
                        key={t.id}
                        data-index={index}
                        ref={virtualized ? tabVirtualizer.measureElement : undefined}
                        className={`tab-wrap${t.active ? " active" : ""}${t.className ? ` ${t.className}` : ""}${reorder.dragClass(t.id)}`}
                        role="presentation">
                        <Tooltip label={t.title}>
                            <button
                                ref={(element) => {
                                    if (element) tabRefs.current.set(t.id, element);
                                    else tabRefs.current.delete(t.id);
                                }}
                                type="button"
                                role="tab"
                                id={t.tabId}
                                aria-controls={t.panelId}
                                aria-selected={t.active ?? false}
                                tabIndex={t.active || (activeIndex < 0 && index === 0) ? 0 : -1}
                                onKeyDown={(event) => {
                                    if (["ArrowLeft", "ArrowRight", "Home", "End"].includes(event.key)) {
                                        event.preventDefault();
                                        const next =
                                            event.key === "Home"
                                                ? 0
                                                : event.key === "End"
                                                  ? tabs.length - 1
                                                  : (index + (event.key === "ArrowRight" ? 1 : -1) + tabs.length) % tabs.length;
                                        focusTabAt(next);
                                    }
                                    if (event.key === "Delete" && closable && onClose) {
                                        event.preventDefault();
                                        const next = tabs[index + 1] ?? tabs[index - 1];
                                        if (virtualized && next) tabVirtualizer.scrollToIndex(tabs.indexOf(next), { align: "auto" });
                                        onClose(t.id);
                                        if (next) requestAnimationFrame(() => tabRefs.current.get(next.id)?.focus({ preventScroll: true }));
                                    }
                                    if (event.shiftKey && event.key === "F10" && buildMenu) {
                                        event.preventDefault();
                                        const rect = event.currentTarget.getBoundingClientRect();
                                        setMenu({ x: rect.left, y: rect.bottom, id: t.id });
                                    }
                                }}
                                aria-label={`${t.label}${t.dirty ? ", unsaved changes" : ""}`}
                                className={`tab${t.active ? " active" : ""}`}
                                onPointerDown={onReorder ? (event) => reorder.onPointerDown(event, t.id) : undefined}
                                onClick={(event) => {
                                    if (reorder.consumeClick()) return;
                                    event.currentTarget.focus({ preventScroll: true });
                                    onSelect(t.id);
                                }}
                                onContextMenu={
                                    buildMenu
                                        ? (e) => {
                                              e.preventDefault();
                                              setMenu({ x: e.clientX, y: e.clientY, id: t.id });
                                          }
                                        : undefined
                                }>
                                {t.icon && <span className="tab-mark">{t.icon}</span>}
                                <span className="tab-label">{t.label}</span>
                                {t.badge && <span className="tab-badge">{t.badge}</span>}
                            </button>
                        </Tooltip>
                        {(status || (closable && onClose)) && (
                            <span className="tab-tail">
                                {status && <span className="tab-status">{status}</span>}
                                {closable && onClose && (
                                    <Tooltip label={`Close ${t.label}`}>
                                        <button type="button" className="tab-x" aria-label={`Close ${t.label}`} onClick={() => onClose(t.id)}>
                                            <IconClose size={11} />
                                        </button>
                                    </Tooltip>
                                )}
                            </span>
                        )}
                    </div>
                );
            })}
            {virtualized && (
                <div aria-hidden="true" style={{ flex: `0 0 ${Math.max(0, tabVirtualizer.getTotalSize() - (lastVirtual?.end ?? 0))}px` }} />
            )}
            {onAdd && (
                <Tooltip label={addTitle}>
                    <button type="button" className="tab-add" aria-label={addLabel ?? addTitle} onClick={onAdd}>
                        {addIcon}
                    </button>
                </Tooltip>
            )}
            {trailing && <div className="tabbar-trailing">{trailing}</div>}
            {menu && menuItems && <TreeContextMenu x={menu.x} y={menu.y} items={menuItems} onClose={() => setMenu(null)} />}
        </div>
    );
}
