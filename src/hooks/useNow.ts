import { useCallback, useState, useSyncExternalStore } from "react";

interface Clock {
    now: number;
    listeners: Set<() => void>;
    timer: number;
}

const clocks = new Map<number, Clock>();

function clockFor(intervalMs: number): Clock {
    let clock = clocks.get(intervalMs);
    if (!clock) {
        clock = { now: Date.now(), listeners: new Set(), timer: 0 };
        clocks.set(intervalMs, clock);
    }
    return clock;
}

function subscribe(clock: Clock, intervalMs: number, listener: () => void): () => void {
    if (clock.listeners.size === 0) {
        const tick = () => {
            clock.now = Date.now();
            for (const notify of clock.listeners) notify();
            clock.timer = window.setTimeout(tick, intervalMs - (Date.now() % intervalMs));
        };
        clock.timer = window.setTimeout(tick, intervalMs - (Date.now() % intervalMs));
    }
    clock.listeners.add(listener);
    return () => {
        clock.listeners.delete(listener);
        if (clock.listeners.size === 0) window.clearTimeout(clock.timer);
    };
}

/** The current time, read again on every whole `intervalMs` of the wall clock while `ticking`. Every view on the same interval shares one timer. */
export function useNow(ticking = true, intervalMs = 1_000): number {
    const [mountedAt] = useState(Date.now);
    const clock = clockFor(intervalMs);
    const subscribeClock = useCallback(
        (listener: () => void) => (ticking ? subscribe(clock, intervalMs, listener) : () => {}),
        [clock, ticking, intervalMs],
    );
    const now = useSyncExternalStore(subscribeClock, () => clock.now);
    return Math.max(mountedAt, now);
}
