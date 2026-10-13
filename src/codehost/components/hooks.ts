import { useCallback, useRef, useState } from "react";

const MINUTE = 60_000;

/**
 * The same clock, rounded to the minute. A row showing "3h ago" re-reads the
 * same string sixty times a minute otherwise, and each reading re-renders it.
 */
export function coarse(now: number): number {
    return Math.floor(now / MINUTE) * MINUTE;
}

/** Runs one piece of work at a time, so a second press while the first is in flight does nothing. */
export function useBusy(): [busy: boolean, run: (work: () => Promise<unknown>) => void] {
    const [busy, setBusy] = useState(false);
    const running = useRef(false);
    const run = useCallback((work: () => Promise<unknown>) => {
        if (running.current) return;
        running.current = true;
        setBusy(true);
        const settle = () => {
            running.current = false;
            setBusy(false);
        };
        void work().then(settle, settle);
    }, []);
    return [busy, run];
}
