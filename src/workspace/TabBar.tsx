import { useVirtualizer } from "@tanstack/react-virtual";
import { useLayoutEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import { animate, type Box, contentBox, EASE_LEAVE, glideSelection, leavingRef, prefersReducedMotion } from "../lib/motion";
import { TreeContextMenu, type CtxItem } from "../rail/FileTree";
import { IconClose } from "../ui/Icons";
import { Tooltip } from "../ui/Tooltip";
import { useTabReorder, type TabDragOut, type TabDropRule, type TabReorderHandler } from "./useTabReorder";

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
    /** Sets the label's text apart from the tab's own colour, as an agent's name is. */
    /** Dim text after the label, such as the name of the agent a tab holds. */
    note?: string;
    /** Tabs next to each other with the same group are outlined together, like the panes of one split. */
    group?: string;
    /** A temporary tab the next preview replaces. Drawn in italics until `onKeep` is called for it. */
    preview?: boolean;
}

export type TabVariant = "editor" | "agent" | "desk" | "stack";

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

/** The element that paints a strip's selection: the whole tab in most strips, the button alone in some. */
function selectionSurface(wrap: HTMLElement): HTMLElement {
    // Read with the tint's own fade stopped, or a just-selected tab still reports its unselected colour.
    const kept = wrap.style.transition;
    wrap.style.transition = "none";
    const style = getComputedStyle(wrap);
    const paints = style.backgroundColor !== "rgba(0, 0, 0, 0)" || style.boxShadow !== "none";
    wrap.style.transition = kept;
    return paints ? wrap : (wrap.querySelector<HTMLElement>(".tab") ?? wrap);
}

/** A strip that changed most of its tabs at once is showing a different set, such as another project's, not opening or closing one. */
function isSwap(seen: Set<string> | null, tabs: TabDescriptor[]): boolean {
    if (!seen) return false;
    const kept = tabs.filter((tab) => seen.has(tab.id)).length;
    const added = tabs.length - kept;
    const removed = seen.size - kept;
    return added > 2 || removed > 2 || (kept === 0 && seen.size > 0 && tabs.length > 0);
}

function openTab(wrap: HTMLElement): void {
    const width = wrap.getBoundingClientRect().width;
    const overflow = wrap.style.overflow;
    wrap.style.overflow = "hidden";
    const run = animate(
        wrap,
        [
            { width: "0px", minWidth: "0px", opacity: 0 },
            { width: `${width}px`, minWidth: "0px", opacity: 1 },
        ],
        { duration: 160 },
    );
    const done = () => (wrap.style.overflow = overflow);
    if (run) run.finished.then(done, done);
    else done();
}

function closeTab(wrap: HTMLElement): Animation | null {
    const width = wrap.getBoundingClientRect().width;
    wrap.style.overflow = "hidden";
    for (const part of wrap.children) animate(part, [{ opacity: 1 }, { opacity: 0 }], { duration: 50, easing: "linear", fill: "forwards" });
    return animate(
        wrap,
        [
            { width: `${width}px`, minWidth: "0px" },
            { width: "0px", minWidth: "0px", paddingLeft: "0px", paddingRight: "0px", marginLeft: "0px", marginRight: "0px" },
        ],
        { duration: 120, easing: EASE_LEAVE },
    );
}

interface TabBarProps {
    variant: TabVariant;
    tabs: TabDescriptor[];
    onSelect: (id: string) => void;
    onClose?: (id: string) => void;
    /** A preview tab was double-clicked, so it should stay. */
    onKeep?: (id: string) => void;
    /** Any other tab was double-clicked, to rename it. */
    onRename?: (id: string) => void;
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
    /** Lets a tab be pulled down out of the strip and dropped on the stage. */
    dragOut?: TabDragOut;
}

export function TabBar({
    variant,
    tabs,
    onSelect,
    onClose,
    onKeep,
    onRename,
    buildMenu,
    onAdd,
    addIcon,
    addTitle,
    addLabel,
    trailing,
    ariaLabel,
    onReorder,
    canReorder,
    dragOut,
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
        dragOut,
    );
    const activeIndex = tabs.findIndex((tab) => tab.active);
    const activeId = tabs[activeIndex]?.id;
    const wrapRefs = useRef(new Map<string, HTMLDivElement>());
    const closedBoxes = useRef(new Map<string, Box>());
    const virtualizedRef = useRef(virtualized);
    virtualizedRef.current = virtualized;
    const shownIds = useRef<Set<string> | null>(null);
    const swappedRef = useRef(false);
    swappedRef.current = isSwap(shownIds.current, tabs);
    const measureRef = useRef(tabVirtualizer.measureElement);
    measureRef.current = tabVirtualizer.measureElement;
    /*
     * One stable ref for every tab: it measures for the virtualizer, keeps the
     * map the selection glide reads, and lets a closed tab narrow away. A
     * virtualized strip unmounts tabs just by scrolling, so there a removed tab
     * simply goes.
     */
    const wrapRef = useMemo(
        () =>
            leavingRef<HTMLDivElement>(closeTab, {
                onMount: (el) => {
                    if (virtualizedRef.current) measureRef.current(el);
                    const id = el.dataset.tabId;
                    if (id) wrapRefs.current.set(id, el);
                },
                onRemove: (el) => {
                    const id = el.dataset.tabId;
                    if (id && wrapRefs.current.get(id) === el) wrapRefs.current.delete(id);
                    const strip = scrollRef.current;
                    if (swappedRef.current) return false;
                    if (id && strip && el.classList.contains("active"))
                        closedBoxes.current.set(id, contentBox(selectionSurface(el).getBoundingClientRect(), strip));
                    return !virtualizedRef.current;
                },
            }),
        [],
    );

    useLayoutEffect(() => {
        if (virtualized && activeIndex >= 0) tabVirtualizer.scrollToIndex(activeIndex, { align: "auto" });
    }, [activeIndex, tabVirtualizer, virtualized]);

    useLayoutEffect(() => {
        const strip = scrollRef.current;
        if (!strip) return;
        // Where the tabs end in layout, not scrollWidth: the selection glide's copy and a dragged tab's
        // transform both reach past the last tab for a moment, and nothing resizes when they go.
        const mark = () => {
            let end = 0;
            for (const child of strip.children) {
                if (!(child instanceof HTMLElement) || child.classList.contains("selection-glide")) continue;
                end = Math.max(end, child.offsetLeft + child.offsetWidth);
            }
            const hidden = end - strip.clientWidth;
            strip.toggleAttribute("data-fade-start", strip.scrollLeft > 1);
            strip.toggleAttribute("data-fade-end", hidden - strip.scrollLeft > 1);
        };
        const resize = new ResizeObserver(mark);
        const watch = () => {
            resize.disconnect();
            resize.observe(strip);
            for (const child of strip.children) resize.observe(child);
            mark();
        };
        watch();
        const children = new MutationObserver(watch);
        children.observe(strip, { childList: true });
        strip.addEventListener("scroll", mark, { passive: true });
        return () => {
            strip.removeEventListener("scroll", mark);
            children.disconnect();
            resize.disconnect();
        };
    }, []);

    useLayoutEffect(() => {
        if (activeId === undefined) return;
        // A virtualized strip may not have mounted the pill yet — the
        // virtualizer above has it roughly in view.
        reveal(scrollRef.current, tabRefs.current.get(activeId));
    }, [activeId]);

    const previousActive = useRef(activeId);
    useLayoutEffect(() => {
        const strip = scrollRef.current;
        const seen = shownIds.current;
        shownIds.current = new Set(tabs.map((tab) => tab.id));
        const from = previousActive.current;
        previousActive.current = activeId;
        if (!strip || !seen || virtualized || reorder.dragging || isSwap(seen, tabs)) {
            closedBoxes.current.clear();
            return;
        }

        // Measure where the selection lands before any new tab starts growing from nothing.
        if (from !== activeId && activeId !== undefined && from !== undefined) {
            const toWrap = wrapRefs.current.get(activeId);
            const fromWrap = wrapRefs.current.get(from);
            if (toWrap) {
                const surface = selectionSurface(toWrap);
                const fromSurface = fromWrap && (surface === toWrap ? fromWrap : fromWrap.querySelector<HTMLElement>(".tab"));
                const fromBox = fromSurface ? contentBox(fromSurface.getBoundingClientRect(), strip) : closedBoxes.current.get(from);
                if (fromBox) glideSelection(strip, fromBox, surface, fromSurface);
            }
        }
        closedBoxes.current.clear();

        for (const tab of tabs.filter((tab) => !seen.has(tab.id))) {
            const wrap = wrapRefs.current.get(tab.id);
            if (wrap) openTab(wrap);
        }
    });

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

    const renderTab = (t: TabDescriptor, index: number) => {
        const closable = t.closable ?? !!onClose;
        // One mark at a time: a tab that is busy says so, a tab that is
        // only unsaved shows the dot.
        const status = t.accessory ?? (t.dirty ? <span className="tab-dot" aria-hidden="true" /> : null);
        return (
            <div
                key={t.id}
                data-index={index}
                data-tab-id={t.id}
                ref={wrapRef}
                className={`tab-wrap${t.active ? " active" : ""}${t.className ? ` ${t.className}` : ""}${reorder.dragClass(t.id)}`}
                role="presentation"
                data-no-window-drag
                onPointerDown={
                    onReorder
                        ? (event) => {
                              if (!(event.target instanceof Element && event.target.closest(".tab-x"))) reorder.onPointerDown(event, t.id);
                          }
                        : undefined
                }>
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
                        className={`tab${t.active ? " active" : ""}${t.preview ? " preview" : ""}`}
                        onClick={(event) => {
                            if (reorder.consumeClick()) return;
                            event.currentTarget.focus({ preventScroll: true });
                            onSelect(t.id);
                        }}
                        onDoubleClick={t.preview && onKeep ? () => onKeep(t.id) : onRename ? () => onRename(t.id) : undefined}
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
                        {t.note && <span className="tab-note">{t.note}</span>}
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
    };

    const runs: { group?: string; items: { tab: TabDescriptor; index: number }[] }[] = [];
    for (const item of visibleTabs) {
        const last = runs[runs.length - 1];
        if (!virtualized && item.tab.group && last?.group === item.tab.group) last.items.push(item);
        else runs.push({ group: virtualized ? undefined : item.tab.group, items: [item] });
    }

    return (
        <div className={`tabbar v-${variant}${reorder.dragging ? " is-reordering" : ""}`}>
            <div ref={scrollRef} className="tabbar-tabs" role="tablist" aria-label={ariaLabel}>
                {virtualized && <div aria-hidden="true" style={{ flex: `0 0 ${firstVirtual?.start ?? 0}px` }} />}
                {runs.map((run) =>
                    run.group && run.items.length > 1 ? (
                        <div
                            key={`group:${run.group}`}
                            className={`tab-group${run.items.some(({ tab }) => tab.active) ? " active" : ""}`}
                            role="presentation"
                            data-no-window-drag>
                            {run.items.map(({ tab, index }) => renderTab(tab, index))}
                        </div>
                    ) : (
                        run.items.map(({ tab, index }) => renderTab(tab, index))
                    ),
                )}
                {virtualized && (
                    <div aria-hidden="true" style={{ flex: `0 0 ${Math.max(0, tabVirtualizer.getTotalSize() - (lastVirtual?.end ?? 0))}px` }} />
                )}
            </div>
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
