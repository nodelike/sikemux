import { useCallback, useEffect, useRef, useState, type PointerEvent as ReactPointerEvent, type RefObject } from "react";
import { flushSync } from "react-dom";
import { prefersReducedMotion } from "../lib/motion";
import {
    allowedSlot,
    clampTravel,
    dropForSlot,
    settleOffset,
    slideFor,
    slotAt,
    TAB_DRAG_THRESHOLD,
    TAB_SLIDE_MS,
    type TabBox,
    type TabPlacement,
} from "./tabDrag";

export type TabReorderHandler = (sourceId: string, targetId: string, placement: TabPlacement) => void;
export type TabDropRule = (sourceId: string, targetId: string, placement: TabPlacement) => boolean;

interface DragSession {
    sourceId: string;
    startX: number;
    startY: number;
    active: boolean;
    from: number;
    slot: number;
    width: number;
    boxes: TabBox[];
    pills: HTMLElement[];
}

const shift = (px: number) => (px === 0 ? "" : `translateX(${px}px)`);

/**
 * Press-and-drag reordering for a tab strip, moved on screen as it happens:
 * the held tab follows the pointer and its neighbours slide aside to open the
 * gap it will drop into. Nothing moves until the pointer travels a few pixels,
 * so a click still only selects, and Escape puts everything back.
 *
 * Positions are written straight onto the pills during the drag; React only
 * hears about the start, the end and the new order.
 */
export function useTabReorder(
    tabElements: RefObject<Map<string, HTMLElement>>,
    orderedIds: readonly string[],
    onReorder: TabReorderHandler | undefined,
    canDrop: TabDropRule | undefined,
) {
    const [draggingId, setDraggingId] = useState<string | null>(null);
    const session = useRef<DragSession | null>(null);
    const detach = useRef<(() => void) | null>(null);
    const settleTimer = useRef<number | null>(null);
    const swallowClick = useRef(false);
    const latest = useRef({ orderedIds, onReorder, canDrop });
    latest.current = { orderedIds, onReorder, canDrop };

    /* Clears every offset in one frame with transitions off, so tabs land
       where they already are instead of animating back from it. */
    const release = useCallback((pills: readonly HTMLElement[], apply?: () => void) => {
        for (const pill of pills) pill.style.transition = "none";
        if (apply) flushSync(apply);
        for (const pill of pills) pill.style.transform = "";
        void pills[0]?.offsetWidth;
        for (const pill of pills) pill.style.transition = "";
    }, []);

    const end = useCallback(() => {
        detach.current?.();
        detach.current = null;
        if (settleTimer.current !== null) window.clearTimeout(settleTimer.current);
        settleTimer.current = null;
        const drag = session.current;
        session.current = null;
        if (drag?.active) release(drag.pills);
        setDraggingId(null);
        document.body.classList.remove("is-sorting-tabs");
    }, [release]);

    useEffect(() => end, [end]);

    const measure = (sourceId: string): Pick<DragSession, "from" | "boxes" | "pills" | "width"> | null => {
        const pills: HTMLElement[] = [];
        const boxes: TabBox[] = [];
        for (const id of latest.current.orderedIds) {
            const tab = tabElements.current?.get(id);
            const pill = (tab?.closest(".tab-wrap") as HTMLElement | null) ?? tab;
            if (!pill) continue;
            const { left, right } = pill.getBoundingClientRect();
            pills.push(pill);
            boxes.push({ id, left, right });
        }
        const from = boxes.findIndex((box) => box.id === sourceId);
        if (from < 0) return null;
        return { from, boxes, pills, width: boxes[from].right - boxes[from].left };
    };

    const layOut = (drag: DragSession, dx: number) => {
        drag.pills[drag.from].style.transform = shift(dx);
        const rule = latest.current.canDrop ?? (() => true);
        const slot = allowedSlot(drag.boxes, drag.from, slotAt(drag.boxes, drag.from, dx), rule);
        if (slot === drag.slot) return;
        drag.slot = slot;
        drag.pills.forEach((pill, index) => {
            if (index !== drag.from) pill.style.transform = shift(slideFor(index, drag.from, slot, drag.width));
        });
    };

    const onPointerDown = (event: ReactPointerEvent, sourceId: string) => {
        if (!latest.current.onReorder || event.button !== 0) return;
        end();
        session.current = {
            sourceId,
            startX: event.clientX,
            startY: event.clientY,
            active: false,
            from: -1,
            slot: -1,
            width: 0,
            boxes: [],
            pills: [],
        };

        const move = (e: PointerEvent) => {
            const drag = session.current;
            if (!drag) return;
            if (!drag.active) {
                if (Math.hypot(e.clientX - drag.startX, e.clientY - drag.startY) < TAB_DRAG_THRESHOLD) return;
                const measured = measure(drag.sourceId);
                if (!measured) return end();
                Object.assign(drag, measured, { active: true, slot: measured.from });
                setDraggingId(drag.sourceId);
                document.body.classList.add("is-sorting-tabs");
            }
            e.preventDefault();
            layOut(drag, clampTravel(drag.boxes, drag.from, e.clientX - drag.startX));
        };

        const drop = () => {
            const drag = session.current;
            if (!drag?.active) return end();
            // The browser still delivers a click to the pill the drag began on.
            swallowClick.current = true;
            window.setTimeout(() => (swallowClick.current = false), 0);
            detach.current?.();
            detach.current = null;

            const target = dropForSlot(drag.boxes, drag.from, drag.slot);
            const held = drag.pills[drag.from];
            held.classList.add("tab-settling");
            held.style.transform = shift(settleOffset(drag.boxes, drag.from, drag.slot));
            const finish = () => {
                settleTimer.current = null;
                held.classList.remove("tab-settling");
                session.current = null;
                const commit = target ? () => latest.current.onReorder?.(drag.sourceId, target.targetId, target.placement) : undefined;
                release(drag.pills, commit);
                setDraggingId(null);
                document.body.classList.remove("is-sorting-tabs");
            };
            if (prefersReducedMotion()) finish();
            else settleTimer.current = window.setTimeout(finish, TAB_SLIDE_MS);
        };

        const cancelOnEscape = (e: KeyboardEvent) => {
            const drag = session.current;
            if (e.key !== "Escape" || !drag?.active) return;
            e.preventDefault();
            e.stopPropagation();
            drag.slot = drag.from;
            drag.pills.forEach((pill, index) => {
                if (index !== drag.from) pill.style.transform = "";
            });
            drop();
        };

        window.addEventListener("pointermove", move);
        window.addEventListener("pointerup", drop);
        window.addEventListener("pointercancel", end);
        window.addEventListener("keydown", cancelOnEscape, true);
        detach.current = () => {
            window.removeEventListener("pointermove", move);
            window.removeEventListener("pointerup", drop);
            window.removeEventListener("pointercancel", end);
            window.removeEventListener("keydown", cancelOnEscape, true);
        };
    };

    /** True when this click is the tail of a drag and should not select. */
    const consumeClick = (): boolean => {
        if (!swallowClick.current) return false;
        swallowClick.current = false;
        return true;
    };

    const dragClass = (id: string): string => (draggingId === id ? " tab-lifted" : "");

    return { onPointerDown, consumeClick, dragClass, dragging: draggingId !== null };
}
