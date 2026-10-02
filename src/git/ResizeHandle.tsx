import type { PointerEvent as ReactPointerEvent, RefObject } from "react";

export const LEFT_MIN = 260;
export const RIGHT_MIN = 360;
export const HISTORY_MIN = 96;
/** Room kept above an open history for the tabs, the commit box and a few file rows. */
export const HISTORY_CLEARANCE = 280;

/**
 * A hairline that resizes the box beside it. Dragging sizes the box directly and saves on release;
 * `grows` says which way along the axis makes the box bigger.
 */
export function ResizeHandle({
    targetRef,
    axis,
    grows,
    min,
    max,
    size,
    label,
    className,
    onResize,
}: {
    targetRef: RefObject<HTMLDivElement | null>;
    axis: "x" | "y";
    grows: 1 | -1;
    min: number;
    max: () => number;
    size: number | null;
    label: string;
    className: string;
    onResize: (size: number | null) => void;
}) {
    const clamp = (next: number) => Math.round(Math.min(Math.max(min, max()), Math.max(min, next)));
    const measure = () => (axis === "x" ? targetRef.current?.offsetWidth : targetRef.current?.offsetHeight) ?? size ?? min;
    const apply = (next: number) => {
        const el = targetRef.current;
        if (!el) return;
        if (axis === "x") el.style.width = `${next}px`;
        else {
            el.style.flex = `0 0 ${next}px`;
            el.style.minHeight = `${min}px`;
        }
    };

    const onPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
        if (event.button !== 0) return;
        event.preventDefault();
        const handle = event.currentTarget;
        handle.setPointerCapture(event.pointerId);
        const start = axis === "x" ? event.clientX : event.clientY;
        const startSize = measure();
        let latest = startSize;
        let frame: number | null = null;
        const move = (ev: PointerEvent) => {
            latest = clamp(startSize + grows * ((axis === "x" ? ev.clientX : ev.clientY) - start));
            if (frame !== null) return;
            frame = window.requestAnimationFrame(() => {
                frame = null;
                apply(latest);
            });
        };
        const up = () => {
            if (frame !== null) window.cancelAnimationFrame(frame);
            handle.removeEventListener("pointermove", move);
            handle.removeEventListener("pointerup", up);
            handle.removeEventListener("pointercancel", up);
            if (latest !== startSize) onResize(latest);
        };
        handle.addEventListener("pointermove", move);
        handle.addEventListener("pointerup", up);
        handle.addEventListener("pointercancel", up);
    };

    const [less, more] = axis === "x" ? ["ArrowLeft", "ArrowRight"] : grows > 0 ? ["ArrowUp", "ArrowDown"] : ["ArrowDown", "ArrowUp"];
    return (
        <div
            className={className}
            role="separator"
            aria-orientation={axis === "x" ? "vertical" : "horizontal"}
            aria-label={label}
            aria-valuemin={min}
            aria-valuenow={size ?? undefined}
            tabIndex={0}
            title="Drag to resize · double-click to reset"
            onPointerDown={onPointerDown}
            onDoubleClick={() => {
                if (targetRef.current) {
                    targetRef.current.style.flex = "";
                    targetRef.current.style.minHeight = "";
                }
                onResize(null);
            }}
            onKeyDown={(event) => {
                if (event.key !== less && event.key !== more) return;
                event.preventDefault();
                event.stopPropagation();
                const step = event.shiftKey ? 64 : 16;
                onResize(clamp(measure() + (event.key === more ? step : -step)));
            }}
        />
    );
}
