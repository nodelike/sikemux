import { useEffect, useRef, useSyncExternalStore } from "react";

export interface ResourceDef<Args extends unknown[], T> {
    kind: string;
    fetch: (...args: Args) => Promise<T>;
    staleAfterMs?: number;
    keyFn?: (args: Args) => string;
}

type Status = "loading" | "ok" | "error";

interface Entry<T> {
    kind: string;
    args: unknown[];
    status: Status;
    data?: T;
    error?: string;
    fetchedAt: number;
}

type AnyDef = ResourceDef<unknown[], unknown>;

interface Inflight {
    generation: number;
    promise: Promise<unknown>;
    queued?: Promise<unknown>;
}

const cache = new Map<string, Entry<unknown>>();
const inflight = new Map<string, Inflight>();
const generations = new Map<string, number>();
const subs = new Map<string, Set<() => void>>();
const defsByKind = new Map<string, AnyDef>();

const MAX_CACHE_ENTRIES = 256;
const UNUSED_ENTRY_TTL_MS = 15 * 60_000;

function releaseKeyIfUnused(key: string): void {
    if (cache.has(key) || inflight.has(key) || subs.has(key)) return;
    generations.delete(key);
}

function deleteCacheEntry(key: string): void {
    cache.delete(key);
    releaseKeyIfUnused(key);
}

function pruneCache(): void {
    const now = Date.now();
    for (const [key, entry] of cache) {
        if (subs.has(key) || inflight.has(key) || entry.status === "loading") continue;
        if (now - entry.fetchedAt > UNUSED_ENTRY_TTL_MS) deleteCacheEntry(key);
    }
    if (cache.size <= MAX_CACHE_ENTRIES) return;
    const evictable = [...cache]
        .filter(([key, entry]) => !subs.has(key) && !inflight.has(key) && entry.status !== "loading")
        .sort((a, b) => a[1].fetchedAt - b[1].fetchedAt);
    for (const [key] of evictable) {
        if (cache.size <= MAX_CACHE_ENTRIES) break;
        deleteCacheEntry(key);
    }
}

function framed(tag: string, value: string): string {
    return `${tag}${value.length}:${value}`;
}

function encodeKey(value: unknown, seen: Set<object>): string {
    if (value === null) return "l";
    if (value === undefined) return "u";
    if (typeof value === "string") return framed("s", value);
    if (typeof value === "boolean") return value ? "b1" : "b0";
    if (typeof value === "number") {
        const rendered = Number.isNaN(value) ? "NaN" : Object.is(value, -0) ? "-0" : String(value);
        return framed("n", rendered);
    }
    if (typeof value === "bigint") return framed("i", String(value));
    if (typeof value !== "object") throw new TypeError(`unsupported resource argument type: ${typeof value}`);
    if (seen.has(value)) throw new TypeError("resource arguments must not be cyclic");
    seen.add(value);
    let encoded: string;
    if (Array.isArray(value)) {
        const indexedKeys = Object.keys(value).filter((key) => /^(0|[1-9]\d*)$/.test(key) && Number(key) < value.length);
        if (indexedKeys.length !== Object.keys(value).length || Object.getOwnPropertySymbols(value).length > 0) {
            throw new TypeError("resource argument arrays must not have custom properties");
        }
        let items = "";
        for (let index = 0; index < value.length; index += 1) {
            items += Object.prototype.hasOwnProperty.call(value, index) ? framed("e", encodeKey(value[index], seen)) : framed("h", "");
        }
        encoded = framed("a", items);
    } else {
        const proto = Object.getPrototypeOf(value);
        if ((proto !== Object.prototype && proto !== null) || Object.getOwnPropertySymbols(value).length > 0) {
            throw new TypeError("resource arguments must contain only plain objects and arrays");
        }
        encoded = framed(
            "o",
            Object.keys(value as Record<string, unknown>)
                .sort()
                .map((key) => framed("k", key) + framed("v", encodeKey((value as Record<string, unknown>)[key], seen)))
                .join(""),
        );
    }
    seen.delete(value);
    return encoded;
}

function defaultKey(args: unknown[]): string {
    return encodeKey(args, new Set());
}

function keyOf(kind: string, args: unknown[], def?: AnyDef): string {
    const k = def?.keyFn ? def.keyFn(args as never) : defaultKey(args);
    return framed("r", kind) + framed("k", k);
}

function stringifyError(err: unknown): string {
    if (err == null) return "";
    if (typeof err === "string") return err;
    if (err instanceof Error) return err.message;
    if (typeof err === "object") {
        const e = err as { message?: unknown; category?: unknown };
        if (typeof e.message === "string" && e.message) return e.message;
        if (typeof e.category === "string" && e.category) return e.category;
        try {
            return JSON.stringify(err);
        } catch {
            return "unknown error";
        }
    }
    return String(err);
}

function notify(key: string): void {
    subs.get(key)?.forEach((fn) => fn());
}

function setEntry(key: string, entry: Entry<unknown>): void {
    cache.set(key, entry);
    notify(key);
    if (entry.status !== "loading") pruneCache();
}

function trigger<Args extends unknown[], T>(def: ResourceDef<Args, T>, key: string, args: Args): Promise<T> {
    const generation = generations.get(key) ?? 0;
    const existing = inflight.get(key);
    if (existing?.generation === generation) return existing.promise as Promise<T>;
    if (existing) {
        existing.queued ??= existing.promise.catch(() => undefined).then(() => trigger(def, key, args));
        return existing.queued as Promise<T>;
    }
    const current = cache.get(key) as Entry<T> | undefined;
    setEntry(key, {
        kind: def.kind,
        args,
        status: "loading",
        data: current?.data,
        fetchedAt: current?.fetchedAt ?? 0,
    });
    const p = def
        .fetch(...args)
        .then((data) => {
            if ((generations.get(key) ?? 0) === generation) {
                setEntry(key, {
                    kind: def.kind,
                    args,
                    status: "ok",
                    data,
                    fetchedAt: Date.now(),
                });
            }
            return data;
        })
        .catch((err: unknown) => {
            if ((generations.get(key) ?? 0) === generation) {
                setEntry(key, {
                    kind: def.kind,
                    args,
                    status: "error",
                    data: current?.data,
                    error: stringifyError(err),
                    fetchedAt: Date.now(),
                });
            }
            throw err;
        })
        .finally(() => {
            if (inflight.get(key)?.promise === p) inflight.delete(key);
            releaseKeyIfUnused(key);
        });
    inflight.set(key, { generation, promise: p });
    return p;
}

export function resource<Args extends unknown[], T>(def: ResourceDef<Args, T>): ResourceDef<Args, T> {
    defsByKind.set(def.kind, def as unknown as AnyDef);
    return def;
}

export interface ResourceHandle<T> {
    data: T | undefined;
    status: Status;
    error: string | undefined;
    refresh: () => Promise<void>;
}

export function useResourceEnabled<Args extends unknown[], T>(enabled: boolean, def: ResourceDef<Args, T>, ...args: Args): ResourceHandle<T> {
    return useResourceHandle(enabled, false, def, args);
}

export function useCachedResourceEnabled<Args extends unknown[], T>(enabled: boolean, def: ResourceDef<Args, T>, ...args: Args): ResourceHandle<T> {
    return useResourceHandle(enabled, true, def, args);
}

function useResourceHandle<Args extends unknown[], T>(
    enabled: boolean,
    retainCached: boolean,
    def: ResourceDef<Args, T>,
    args: Args,
): ResourceHandle<T> {
    const key = enabled || retainCached ? keyOf(def.kind, args as unknown[], def as unknown as AnyDef) : "";

    const retained = useRef<{ key: string; data: T | undefined }>({ key, data: undefined });
    if (retained.current.key !== key) retained.current = { key, data: undefined };

    useEffect(() => {
        if (!enabled) return;
        const entry = cache.get(key);
        const stale = !entry || (def.staleAfterMs != null && Date.now() - entry.fetchedAt > def.staleAfterMs);
        if (stale) {
            void trigger(def, key, args).catch(() => {});
        }
        // eslint-disable-next-line react-hooks/exhaustive-deps
    }, [enabled, key]);

    const entry = useSyncExternalStore(
        (cb) => {
            if (!enabled) return () => {};
            let set = subs.get(key);
            if (!set) {
                set = new Set();
                subs.set(key, set);
            }
            set.add(cb);
            return () => {
                set!.delete(cb);
                if (set!.size === 0) {
                    subs.delete(key);
                    releaseKeyIfUnused(key);
                }
            };
        },
        () => (enabled || retainCached ? (cache.get(key) as Entry<T> | undefined) : undefined),
        () => undefined,
    );

    if (retainCached && entry?.data !== undefined) retained.current.data = entry.data;

    return {
        data: entry?.data ?? (retainCached ? retained.current.data : undefined),
        status: entry?.status ?? "loading",
        error: entry?.error,
        refresh: () => (enabled ? trigger(def, key, args).then(() => {}) : Promise.resolve()),
    };
}

export function useResource<Args extends unknown[], T>(def: ResourceDef<Args, T>, ...args: Args): ResourceHandle<T> {
    return useResourceEnabled(true, def, ...args);
}

export function fetchResource<Args extends unknown[], T>(def: ResourceDef<Args, T>, ...args: Args): Promise<T> {
    return trigger(def, keyOf(def.kind, args as unknown[], def as unknown as AnyDef), args);
}

export function peekResource<Args extends unknown[], T>(def: ResourceDef<Args, T>, ...args: Args): T | undefined {
    const e = cache.get(keyOf(def.kind, args as unknown[], def as unknown as AnyDef)) as Entry<T> | undefined;
    return e?.data;
}

export function invalidate(predicate: (kind: string, args: unknown[]) => boolean): void {
    for (const [key, entry] of [...cache]) {
        if (!predicate(entry.kind, entry.args)) continue;
        generations.set(key, (generations.get(key) ?? 0) + 1);
        if (subs.has(key)) {
            const def = defsByKind.get(entry.kind);
            if (def) {
                void trigger(def, key, entry.args).catch(() => {});
            } else {
                deleteCacheEntry(key);
                notify(key);
            }
        } else {
            deleteCacheEntry(key);
            notify(key);
        }
    }
    pruneCache();
}

export function resetResourcesForTests(): void {
    cache.clear();
    inflight.clear();
    generations.clear();
    subs.clear();
    defsByKind.clear();
}

export function resourceStats(): { cacheEntries: number; inflight: number; subscribedKeys: number; subscriptions: number } {
    let subscriptions = 0;
    for (const set of subs.values()) subscriptions += set.size;
    return {
        cacheEntries: cache.size,
        inflight: inflight.size,
        subscribedKeys: subs.size,
        subscriptions,
    };
}
