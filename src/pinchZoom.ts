import { useEffect } from "react";
import * as cmd from "./state/commands";
import { getState } from "./state/store";

interface TextSurface {
    size: () => number;
    setSize: (value: number) => void;
}

const SURFACES: readonly [selector: string, surface: TextSurface][] = [
    [".agent-chat-pane", { size: () => getState().chatTextScale, setSize: cmd.setChatTextScale }],
    [".cm-editor", { size: () => getState().editorTextScale, setSize: cmd.setEditorTextScale }],
    [".terminal-shell", { size: () => getState().terminalFontSize, setSize: cmd.setTerminalFontSize }],
];

/** How long a pause in ctrl-wheel events ends one pinch and starts the next. */
export const PINCH_IDLE_MS = 300;

/** Ctrl-wheel distance that doubles or halves the size. */
const WHEEL_DOUBLING = 100;

export function textSurfaceAt(target: EventTarget | null): TextSurface | null {
    if (!(target instanceof Element)) return null;
    return SURFACES.find(([selector]) => target.closest(selector))?.[1] ?? null;
}

interface Pinch {
    surface: TextSurface;
    base: number;
    factor: number;
}

// WebKit reports a trackpad pinch as these gesture events, with the pinch ratio in `scale`.
interface WebKitGestureEvent extends UIEvent {
    scale: number;
}

/**
 * Pinching on the trackpad over a chat, an editor or a terminal resizes its
 * text, like ⌘+ and ⌘− do for whichever of them has focus.
 */
export function installPinchZoom(target: Window = window): () => void {
    let pinch: Pinch | null = null;
    let idleTimer: number | undefined;

    const begin = (eventTarget: EventTarget | null): Pinch | null => {
        const surface = textSurfaceAt(eventTarget);
        return surface ? { surface, base: surface.size(), factor: 1 } : null;
    };
    const resize = (current: Pinch) => current.surface.setSize(current.base * current.factor);

    const onGestureStart = (event: Event) => {
        pinch = begin(event.target);
        if (pinch) event.preventDefault();
    };
    const onGestureChange = (event: Event) => {
        if (!pinch) return;
        event.preventDefault();
        pinch.factor = (event as WebKitGestureEvent).scale;
        resize(pinch);
    };
    const onGestureEnd = () => {
        pinch = null;
    };

    // Chromium and Firefox report the same pinch as wheel events with ctrlKey set.
    const onWheel = (event: WheelEvent) => {
        if (!event.ctrlKey) return;
        const current = pinch ?? begin(event.target);
        if (!current) return;
        event.preventDefault();
        pinch = current;
        current.factor *= 2 ** (-event.deltaY / WHEEL_DOUBLING);
        resize(current);
        window.clearTimeout(idleTimer);
        idleTimer = window.setTimeout(() => (pinch = null), PINCH_IDLE_MS);
    };

    target.addEventListener("gesturestart", onGestureStart, true);
    target.addEventListener("gesturechange", onGestureChange, true);
    target.addEventListener("gestureend", onGestureEnd, true);
    target.addEventListener("wheel", onWheel, { capture: true, passive: false });
    return () => {
        window.clearTimeout(idleTimer);
        target.removeEventListener("gesturestart", onGestureStart, true);
        target.removeEventListener("gesturechange", onGestureChange, true);
        target.removeEventListener("gestureend", onGestureEnd, true);
        target.removeEventListener("wheel", onWheel, { capture: true });
    };
}

export function usePinchZoom(): void {
    useEffect(() => installPinchZoom(), []);
}
