import { getIpcTransport } from "../api/transport";

/** Named on the Rust side, where the scroll events macOS builds are watched. */
const TOUCH_EVENT = "wheel-touch";

/** Neither, until something reports a hand. Nowhere but macOS ever does. */
let down: boolean | null = null;
const watchers = new Set<(down: boolean) => void>();

/** Whether a hand is on the trackpad, or nothing if nobody is watching for one. */
export const fingersDown = (): boolean | null => down;

let watching: AbortController | null = null;

/** Runs the moment a hand lands or leaves, which is what starts and ends a swipe. */
export function onFingers(listener: (down: boolean) => void): () => void {
    watchers.add(listener);
    if (!watching) {
        watching = new AbortController();
        getIpcTransport()
            .subscribe<boolean>(TOUCH_EVENT, (event) => setFingersDown(event.payload === true), { signal: watching.signal })
            .catch(() => setFingersDown(null));
    }
    return () => {
        watchers.delete(listener);
        if (watchers.size > 0 || !watching) return;
        watching.abort();
        watching = null;
    };
}

export function setFingersDown(next: boolean | null): void {
    if (down === next) return;
    down = next;
    if (down !== null) for (const listener of [...watchers]) listener(down);
}
