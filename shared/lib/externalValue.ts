import { useSyncExternalStore } from "react";

export interface ExternalValue<T> {
    get(): T;
    set(next: T): void;
    subscribe(listener: () => void): () => void;
}

/** A value kept outside React that views read with useExternalValue. `watch` runs while anything is subscribed; what it returns stops it. */
export function externalValue<T>(initial: T, watch?: () => () => void): ExternalValue<T> {
    let value = initial;
    let stopWatching: (() => void) | null = null;
    const listeners = new Set<() => void>();
    return {
        get: () => value,
        set(next) {
            if (Object.is(next, value)) return;
            value = next;
            for (const listener of [...listeners]) listener();
        },
        subscribe(listener) {
            listeners.add(listener);
            if (watch) stopWatching ??= watch();
            return () => {
                listeners.delete(listener);
                if (listeners.size || !stopWatching) return;
                const stop = stopWatching;
                stopWatching = null;
                stop();
            };
        },
    };
}

export function useExternalValue<T>(store: ExternalValue<T>): T;
export function useExternalValue<T, S>(store: ExternalValue<T>, select: (value: T) => S): S;
export function useExternalValue<T, S>(store: ExternalValue<T>, select?: (value: T) => S): T | S {
    return useSyncExternalStore(store.subscribe, () => (select ? select(store.get()) : store.get()));
}
