import { useVirtualizer } from "@tanstack/react-virtual";
import { useLayoutEffect, useRef, useState } from "react";
import type { CSSProperties, ReactNode } from "react";
import { TreeContextMenu, type CtxItem } from "./FileTree";
import { IconClose } from "./Icons";
import { Tooltip } from "./Tooltip";

/**
 * One normalized tab. Every tab strip in the app (editor files, agents,
 * terminals, Bruno requests) describes its tabs as these, so selection,
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
    /** Extra control rendered just before the close button (e.g. a per-tab status badge). */
    accessory?: ReactNode;
}

export type TabVariant = "editor" | "agent" | "bruno";

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
    trailing?: ReactNode;
    style?: CSSProperties;
}

export function TabBar({ variant, tabs, onSelect, onClose, buildMenu, onAdd, addIcon, addTitle, trailing, style }: TabBarProps) {
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
    const activeIndex = tabs.findIndex((tab) => tab.active);

    useLayoutEffect(() => {
        if (virtualized && activeIndex >= 0) tabVirtualizer.scrollToIndex(activeIndex, { align: "auto" });
    }, [activeIndex, tabVirtualizer, virtualized]);

    const focusTabAt = (index: number) => {
        const tab = tabs[index];
        if (!tab) return;
        onSelect(tab.id);
        if (virtualized) tabVirtualizer.scrollToIndex(index, { align: "auto" });
        const element = tabRefs.current.get(tab.id);
        if (element) element.focus();
        else requestAnimationFrame(() => tabRefs.current.get(tab.id)?.focus());
    };

    const virtualItems = virtualized ? tabVirtualizer.getVirtualItems() : [];
    const firstVirtual = virtualItems[0];
    const lastVirtual = virtualItems.at(-1);
    const visibleTabs = virtualized
        ? virtualItems.map((item) => ({ tab: tabs[item.index], index: item.index }))
        : tabs.map((tab, index) => ({ tab, index }));

    return (
        <div ref={scrollRef} className={`tabbar v-${variant}`} style={style} role="tablist">
            {virtualized && <div aria-hidden="true" style={{ flex: `0 0 ${firstVirtual?.start ?? 0}px` }} />}
            {visibleTabs.map(({ tab: t, index }) => {
                const closable = t.closable ?? !!onClose;
                return (
                    <div
                        key={t.id}
                        data-index={index}
                        ref={virtualized ? tabVirtualizer.measureElement : undefined}
                        className={`tab-wrap${t.active ? " active" : ""}`}
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
                                tabIndex={t.active || (!tabs.some((tab) => tab.active) && tabs[0] === t) ? 0 : -1}
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
                                        if (next) requestAnimationFrame(() => tabRefs.current.get(next.id)?.focus());
                                    }
                                    if (event.shiftKey && event.key === "F10" && buildMenu) {
                                        event.preventDefault();
                                        const rect = event.currentTarget.getBoundingClientRect();
                                        setMenu({ x: rect.left, y: rect.bottom, id: t.id });
                                    }
                                }}
                                aria-label={`${t.label}${t.dirty ? ", unsaved changes" : ""}`}
                                className={`tab${t.active ? " active" : ""}`}
                                onClick={(event) => {
                                    event.currentTarget.focus();
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
                                {t.icon}
                                <span className="tab-label">{t.label}</span>
                                {t.dirty && <span className="tab-dot" aria-hidden="true" />}
                                {t.accessory}
                            </button>
                        </Tooltip>
                        {closable && onClose && (
                            <Tooltip label={`Close ${t.label}`}>
                                <button type="button" className="tab-x" aria-label={`Close ${t.label}`} onClick={() => onClose(t.id)}>
                                    <IconClose size={11} />
                                </button>
                            </Tooltip>
                        )}
                    </div>
                );
            })}
            {virtualized && (
                <div aria-hidden="true" style={{ flex: `0 0 ${Math.max(0, tabVirtualizer.getTotalSize() - (lastVirtual?.end ?? 0))}px` }} />
            )}
            {onAdd && (
                <Tooltip label={addTitle}>
                    <button type="button" className="tab-add" aria-label={addTitle} onClick={onAdd}>
                        {addIcon}
                    </button>
                </Tooltip>
            )}
            {trailing && <div className="tabbar-trailing">{trailing}</div>}
            {menu && menuItems && <TreeContextMenu x={menu.x} y={menu.y} items={menuItems} onClose={() => setMenu(null)} />}
        </div>
    );
}
