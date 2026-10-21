import { useEffect, useRef, useState, type DependencyList } from "react";
import type { PluginStream } from "./backend";

export { useNow } from "../hooks/useNow";

/** `value` once it has stopped changing for `delayMs`. */
export function useDebounced<T>(value: T, delayMs: number): T {
    const [settled, setSettled] = useState(value);
    useEffect(() => {
        const timer = window.setTimeout(() => setSettled(value), delayMs);
        return () => window.clearTimeout(timer);
    }, [value, delayMs]);
    return settled;
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

/** Reads `handle` again every `ms`, timed as useEvery times it, and not at all while `ms` is null. A read that fails shows on the handle. */
export function useRefreshEvery(handle: { refresh(): Promise<void> }, ms: number | null): void {
    useEvery(ms !== null, ms ?? 0, () => void handle.refresh().catch(() => undefined));
}

export interface StreamLife {
    /** What the stream follows is over: it is not opened again, even when the app shows again. */
    finish(): void;
    /** The stream ended before what it follows did; it opens again after the `retry` delay, if there is one. */
    ended(): void;
}

export interface VisibleStreamOptions {
    /** Close the stream while the app is hidden and open it again once it shows. */
    readonly pauseWhenHidden: boolean;
    /** How long to wait before opening again a stream that `ended`, by how many times in a row it has. */
    readonly retry?: (tries: number) => number;
}

/**
 * Keeps one stream open while `start` is given, opening it again when `deps` change. `start` opens the stream and
 * tells it, through `life`, when it is over for good or ended early.
 */
export function useVisibleStream(start: ((life: StreamLife) => PluginStream) | null, deps: DependencyList, options: VisibleStreamOptions): void {
    const latest = useRef({ start, options });
    latest.current = { start, options };
    const enabled = start !== null;

    useEffect(() => {
        if (!enabled) return;
        let open: PluginStream | null = null;
        let over = false;
        let tries = 0;
        let timer: ReturnType<typeof setTimeout> | undefined;

        const close = () => {
            clearTimeout(timer);
            timer = undefined;
            open?.stop();
            open = null;
        };
        const hidden = () => latest.current.options.pauseWhenHidden && document.hidden;
        const begin = () => {
            const opener = latest.current.start;
            if (over || open || timer !== undefined || !opener || hidden()) return;
            let stream: PluginStream | null = null;
            stream = opener({
                finish: () => {
                    over = true;
                    if (open === stream) close();
                },
                ended: () => {
                    if (open !== stream) return;
                    open = null;
                    const retry = latest.current.options.retry;
                    if (over || !retry) return;
                    timer = setTimeout(() => {
                        timer = undefined;
                        begin();
                    }, retry(tries++));
                },
            });
            open = over ? null : stream;
            if (over) stream.stop();
        };
        const onVisibility = () => {
            if (hidden()) close();
            else begin();
        };

        begin();
        document.addEventListener("visibilitychange", onVisibility);
        return () => {
            over = true;
            document.removeEventListener("visibilitychange", onVisibility);
            close();
        };
        // eslint-disable-next-line react-hooks/exhaustive-deps -- the caller's deps decide when the stream is opened again
    }, [enabled, ...deps]);
}
