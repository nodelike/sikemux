import { useEffect, useState } from "react";
import { filesOn, type FilesHost, type PathKind } from "../api/fs";
import { useRemoteOrigin } from "./origin";

export type PathState = PathKind | "missing";

/* One frame is long enough to gather every reference a screenful of transcript
   asks about, so they go out as one request instead of one each. */
const BATCH_MS = 16;
/* The backend takes at most this many paths per request. */
const BATCH_MAX = 256;
/* A name the agent is about to write is missing when it first mentions it, so
   a miss is only trusted for this long. A hit never goes stale: a tab opened
   for a file deleted since says so itself. */
const MISS_TTL_MS = 5_000;
const MAX_CACHED = 2_000;

interface Known {
    state: PathState;
    at: number;
}

/* Both maps are keyed by the host that holds the path and the path, so another host's files are never taken for this one's. */
const known = new Map<string, Known>();
const waiting = new Map<string, Set<(state: PathState) => void>>();
let flush: ReturnType<typeof setTimeout> | null = null;

const keyOf = (host: FilesHost, path: string) => `${host ?? ""}\0${path}`;

function hostAndPath(key: string): [FilesHost, string] {
    const at = key.indexOf("\0");
    return [key.slice(0, at) || undefined, key.slice(at + 1)];
}

function remember(key: string, state: PathState) {
    known.delete(key);
    known.set(key, { state, at: Date.now() });
    for (const oldest of known.keys()) {
        if (known.size <= MAX_CACHED) break;
        known.delete(oldest);
    }
}

function settle(key: string, state: PathState) {
    remember(key, state);
    const callbacks = waiting.get(key);
    waiting.delete(key);
    for (const callback of callbacks ?? []) callback(state);
}

async function runBatch() {
    flush = null;
    const first = waiting.keys().next().value;
    if (first === undefined) return;
    const [host] = hostAndPath(first);
    const keys = [...waiting.keys()].filter((key) => hostAndPath(key)[0] === host).slice(0, BATCH_MAX);
    try {
        const kinds = await filesOn(host).pathKinds(keys.map((key) => hostAndPath(key)[1]));
        keys.forEach((key, index) => settle(key, kinds[index] ?? "missing"));
    } catch {
        // A lookup that failed is indistinguishable from a file that is not there.
        for (const key of keys) settle(key, "missing");
    }
    if (waiting.size > 0 && flush === null) flush = setTimeout(() => void runBatch(), BATCH_MS);
}

function cached(key: string): PathState | null {
    const held = known.get(key);
    if (!held) return null;
    if (held.state === "missing" && Date.now() - held.at > MISS_TTL_MS) return null;
    return held.state;
}

function ask(key: string, callback: (state: PathState) => void): () => void {
    const callbacks = waiting.get(key) ?? new Set();
    callbacks.add(callback);
    waiting.set(key, callbacks);
    if (flush === null) flush = setTimeout(() => void runBatch(), BATCH_MS);
    return () => {
        callbacks.delete(callback);
        if (callbacks.size === 0 && waiting.get(key) === callbacks) waiting.delete(key);
    };
}

/** Forgets what is known about a path on this host, or about all of them, so the next reference looks again. */
export function forgetPathState(path?: string): void {
    if (path === undefined) known.clear();
    else known.delete(keyOf(undefined, path));
}

/**
 * What a path is, or null until the answer comes back. Lookups are batched
 * across every reference on screen and the answer is kept.
 */
export function usePathState(path: string | null, host?: FilesHost): PathState | null {
    const remote = useRemoteOrigin() !== null;
    const key = path === null ? null : keyOf(host, path);
    const [state, setState] = useState<PathState | null>(() => (key && !remote ? cached(key) : null));
    useEffect(() => {
        if (!key || remote) {
            setState(null);
            return;
        }
        const held = cached(key);
        if (held) {
            setState(held);
            return;
        }
        setState(null);
        let live = true;
        const stop = ask(key, (answer) => {
            if (live) setState(answer);
        });
        return () => {
            live = false;
            stop();
        };
    }, [key, remote]);
    // Another computer's files cannot be looked up here, so what names a path is taken to be one.
    if (path && remote) return path.endsWith("/") ? "dir" : "file";
    return state;
}
