import { useRef, useState, type PointerEvent as ReactPointerEvent } from "react";

const DRAG_START_PX = 5;

/** The column under a point, by the `data-column` its element carries. */
function columnAt(x: number, y: number): string | null {
    return document.elementFromPoint(x, y)?.closest<HTMLElement>("[data-column]")?.dataset.column ?? null;
}

/**
 * Dragging a board's cards between columns with the pointer. A card counts as dragged once it moves a few pixels;
 * `wasDragged` is true for the click that ends a drag, so the card does not also open.
 */
export function useCardDrag(onDrop: (card: string, column: string) => void) {
    const [dragging, setDragging] = useState<{ card: string; over: string | null } | null>(null);
    const justDragged = useRef(false);

    const startDrag = (event: ReactPointerEvent, card: string) => {
        if (event.button !== 0) return;
        event.preventDefault();
        const start = { x: event.clientX, y: event.clientY };
        let moved = false;
        const onMove = (next: PointerEvent) => {
            if (!moved && Math.hypot(next.clientX - start.x, next.clientY - start.y) < DRAG_START_PX) return;
            moved = true;
            setDragging({ card, over: columnAt(next.clientX, next.clientY) });
        };
        const blockNativeDrag = (next: Event) => next.preventDefault();
        const stop = () => {
            window.removeEventListener("pointermove", onMove);
            window.removeEventListener("pointerup", onUp);
            window.removeEventListener("pointercancel", stop);
            window.removeEventListener("dragstart", blockNativeDrag);
            setDragging(null);
        };
        const onUp = (next: PointerEvent) => {
            stop();
            if (!moved) return;
            justDragged.current = true;
            setTimeout(() => (justDragged.current = false), 0);
            const to = columnAt(next.clientX, next.clientY);
            if (to !== null) onDrop(card, to);
        };
        window.addEventListener("pointermove", onMove);
        window.addEventListener("pointerup", onUp);
        window.addEventListener("pointercancel", stop);
        window.addEventListener("dragstart", blockNativeDrag);
    };

    return { dragging, startDrag, wasDragged: () => justDragged.current };
}
