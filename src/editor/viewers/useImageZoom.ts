import { useCallback, useEffect, useRef, useState } from "react";
import type { RefObject } from "react";
import { springHome } from "../../lib/swipe";
import { fingersDown, onFingers } from "../../lib/wheelTouch";

/** Room the fitted picture leaves around itself. */
const MARGIN = 18;
const MIN_SCALE = 0.1;
const MAX_SCALE = 8;
/** Ctrl-wheel distance that doubles or halves the picture. */
const WHEEL_DOUBLING = 100;
/** Where nothing reports the hand, how long ctrl-wheel quiet ends a pinch. */
const PINCH_IDLE_MS = 300;

interface View {
    scale: number;
    x: number;
    y: number;
}

// WebKit reports a trackpad pinch as these gesture events, with the pinch ratio in `scale`.
interface WebKitGestureEvent extends UIEvent {
    scale: number;
    clientX: number;
    clientY: number;
}

export type Zoom = "fit" | number;

/**
 * Pinch to zoom and two-finger pan for a picture inside `stage`. The picture is
 * moved with a transform painted once a frame, so a gesture never re-renders;
 * a pinch left smaller than the fitted size springs back to it once the hand lifts.
 */
export function useImageZoom(
    stageRef: RefObject<HTMLElement | null>,
    imageRef: RefObject<HTMLImageElement | null>,
    dims: { w: number; h: number } | null,
) {
    const [zoom, setZoomState] = useState<Zoom>("fit");
    const view = useRef<View>({ scale: 1, x: 0, y: 0 });
    const fitted = useRef(true);
    const frame = useRef<number | null>(null);
    const stopSpring = useRef<(() => void) | null>(null);

    const fitView = useCallback((): View | null => {
        const stage = stageRef.current;
        if (!stage || !dims) return null;
        const scale = Math.min(1, (stage.clientWidth - 2 * MARGIN) / dims.w, (stage.clientHeight - 2 * MARGIN) / dims.h);
        const safe = Math.max(MIN_SCALE, scale);
        return { scale: safe, x: (stage.clientWidth - dims.w * safe) / 2, y: (stage.clientHeight - dims.h * safe) / 2 };
    }, [stageRef, dims]);

    const clamped = useCallback(
        (next: View): View => {
            const stage = stageRef.current;
            if (!stage || !dims) return next;
            const axis = (at: number, size: number, room: number) => (size <= room ? (room - size) / 2 : Math.min(0, Math.max(room - size, at)));
            return {
                scale: next.scale,
                x: axis(next.x, dims.w * next.scale, stage.clientWidth),
                y: axis(next.y, dims.h * next.scale, stage.clientHeight),
            };
        },
        [stageRef, dims],
    );

    const paint = useCallback(() => {
        frame.current = null;
        const image = imageRef.current;
        const stage = stageRef.current;
        if (!image || !stage || !dims) return;
        const { scale, x, y } = view.current;
        image.style.transform = `translate(${x}px, ${y}px) scale(${scale})`;
        // Inherited, not "visible": a visible child shows through a hidden screen, and stayed over the next tab.
        image.style.visibility = "inherit";
        const pans = dims.w * scale > stage.clientWidth + 0.5 || dims.h * scale > stage.clientHeight + 0.5;
        stage.toggleAttribute("data-pans", pans);
        setZoomState(fitted.current ? "fit" : Math.round(scale * 100) / 100);
    }, [imageRef, stageRef, dims]);

    const show = useCallback(
        (next: View, fit = false) => {
            fitted.current = fit;
            view.current = next;
            frame.current ??= requestAnimationFrame(paint);
        },
        [paint],
    );

    const zoomAt = useCallback(
        (scale: number, clientX: number, clientY: number, from: View = view.current) => {
            const stage = stageRef.current;
            if (!stage) return;
            const box = stage.getBoundingClientRect();
            const px = clientX - box.left;
            const py = clientY - box.top;
            const next = Math.max(MIN_SCALE, Math.min(MAX_SCALE, scale));
            const ratio = next / from.scale;
            show(clamped({ scale: next, x: px - (px - from.x) * ratio, y: py - (py - from.y) * ratio }));
        },
        [stageRef, show, clamped],
    );

    const settle = useCallback(() => {
        const fit = fitView();
        if (!fit || view.current.scale >= fit.scale) return;
        const from = view.current;
        stopSpring.current?.();
        stopSpring.current = springHome(
            1,
            0,
            (t) => show({ scale: fit.scale + (from.scale - fit.scale) * t, x: fit.x + (from.x - fit.x) * t, y: fit.y + (from.y - fit.y) * t }),
            () => {
                stopSpring.current = null;
                show(fit, true);
            },
        );
    }, [fitView, show]);

    const zoomAtCentre = useCallback(
        (scale: number) => {
            const stage = stageRef.current;
            if (!stage) return;
            const box = stage.getBoundingClientRect();
            zoomAt(scale, box.left + box.width / 2, box.top + box.height / 2);
        },
        [stageRef, zoomAt],
    );

    const setZoom = useCallback(
        (next: Zoom) => {
            stopSpring.current?.();
            const fit = fitView();
            if (!fit) return;
            if (next === "fit") show(fit, true);
            else zoomAtCentre(next);
        },
        [fitView, show, zoomAtCentre],
    );

    const zoomBy = useCallback(
        (factor: number) => {
            stopSpring.current?.();
            zoomAtCentre(view.current.scale * factor);
        },
        [zoomAtCentre],
    );

    useEffect(() => {
        const stage = stageRef.current;
        if (!stage || !dims) return;
        let pinch: { base: View; clientX: number; clientY: number } | null = null;
        let idle: number | undefined;

        const onGestureStart = (event: Event) => {
            event.preventDefault();
            stopSpring.current?.();
            const gesture = event as WebKitGestureEvent;
            pinch = { base: view.current, clientX: gesture.clientX, clientY: gesture.clientY };
        };
        const onGestureChange = (event: Event) => {
            if (!pinch) return;
            event.preventDefault();
            zoomAt(pinch.base.scale * (event as WebKitGestureEvent).scale, pinch.clientX, pinch.clientY, pinch.base);
        };
        const onGestureEnd = () => {
            pinch = null;
            settle();
        };

        const onWheel = (event: WheelEvent) => {
            if (event.ctrlKey) {
                event.preventDefault();
                stopSpring.current?.();
                zoomAt(view.current.scale * 2 ** (-event.deltaY / WHEEL_DOUBLING), event.clientX, event.clientY);
                window.clearTimeout(idle);
                if (fingersDown() === null) idle = window.setTimeout(settle, PINCH_IDLE_MS);
                return;
            }
            if (!stage.hasAttribute("data-pans")) return;
            event.preventDefault();
            const { scale, x, y } = view.current;
            show(clamped({ scale, x: x - event.deltaX, y: y - event.deltaY }));
        };

        const onPointerDown = (event: PointerEvent) => {
            if (event.button !== 0 || !stage.hasAttribute("data-pans")) return;
            event.preventDefault();
            stopSpring.current?.();
            stage.setPointerCapture(event.pointerId);
            stage.setAttribute("data-dragging", "");
            let lastX = event.clientX;
            let lastY = event.clientY;
            const onMove = (move: PointerEvent) => {
                const { scale, x, y } = view.current;
                show(clamped({ scale, x: x + move.clientX - lastX, y: y + move.clientY - lastY }));
                lastX = move.clientX;
                lastY = move.clientY;
            };
            const onUp = () => {
                stage.removeAttribute("data-dragging");
                stage.removeEventListener("pointermove", onMove);
                stage.removeEventListener("pointerup", onUp);
                stage.removeEventListener("pointercancel", onUp);
            };
            stage.addEventListener("pointermove", onMove);
            stage.addEventListener("pointerup", onUp);
            stage.addEventListener("pointercancel", onUp);
        };

        const stopFingers = onFingers((down) => {
            if (!down && !pinch) settle();
        });
        const resize = new ResizeObserver(() => {
            const fit = fitView();
            if (!fit) return;
            show(fitted.current ? fit : clamped(view.current), fitted.current);
        });
        resize.observe(stage);
        stage.addEventListener("gesturestart", onGestureStart);
        stage.addEventListener("gesturechange", onGestureChange);
        stage.addEventListener("gestureend", onGestureEnd);
        stage.addEventListener("wheel", onWheel, { passive: false });
        stage.addEventListener("pointerdown", onPointerDown);
        return () => {
            window.clearTimeout(idle);
            stopFingers();
            resize.disconnect();
            stage.removeEventListener("gesturestart", onGestureStart);
            stage.removeEventListener("gesturechange", onGestureChange);
            stage.removeEventListener("gestureend", onGestureEnd);
            stage.removeEventListener("wheel", onWheel);
            stage.removeEventListener("pointerdown", onPointerDown);
        };
    }, [stageRef, dims, fitView, clamped, show, zoomAt, settle]);

    useEffect(
        () => () => {
            stopSpring.current?.();
            if (frame.current != null) cancelAnimationFrame(frame.current);
        },
        [],
    );

    return { zoom, setZoom, zoomBy };
}
