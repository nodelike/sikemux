import { useCallback, useEffect, useRef, useState } from "react";

const MINUTE = 60_000;

/** A clock that only ticks while something on screen is still moving. */
export function useNow(live: boolean): number {
    const [now, setNow] = useState(() => Date.now());
    useEffect(() => {
        if (!live) return;
        const timer = setInterval(() => setNow(Date.now()), 1000);
        return () => clearInterval(timer);
    }, [live]);
    return now;
}

/**
 * The same clock, rounded to the minute. A row showing "3h ago" re-reads the
 * same string sixty times a minute otherwise, and each reading re-renders it.
 */
export function coarse(now: number): number {
    return Math.floor(now / MINUTE) * MINUTE;
}

/**
 * Calls `work` every `ms` while `enabled`. The latest `work` is always the one
 * called, and a render in between does not start the wait over, so a view
 * that redraws every second still refreshes on time. Nothing is read while
 * the window is hidden, and it reads once as soon as it is shown again.
 */
export function useEvery(enabled: boolean, ms: number, work: () => void): void {
    const latest = useRef(work);
    latest.current = work;
    useEffect(() => {
        if (!enabled) return;
        const tick = () => {
            if (!document.hidden) latest.current();
        };
        const timer = setInterval(tick, ms);
        document.addEventListener("visibilitychange", tick);
        return () => {
            clearInterval(timer);
            document.removeEventListener("visibilitychange", tick);
        };
    }, [enabled, ms]);
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
