import { useEffect, useState } from "react";
import { fsapi, type PathKind } from "../api/fs";
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

const known = new Map<string, Known>();
const waiting = new Map<string, Set<(state: PathState) => void>>();
let flush: ReturnType<typeof setTimeout> | null = null;

function remember(path: string, state: PathState) {
    known.delete(path);
    known.set(path, { state, at: Date.now() });
    for (const oldest of known.keys()) {
        if (known.size <= MAX_CACHED) break;
        known.delete(oldest);
    }
}

function settle(path: string, state: PathState) {
    remember(path, state);
    const callbacks = waiting.get(path);
    waiting.delete(path);
    for (const callback of callbacks ?? []) callback(state);
}

async function runBatch() {
    flush = null;
    const paths = [...waiting.keys()].slice(0, BATCH_MAX);
    if (paths.length === 0) return;
    try {
        const kinds = await fsapi.pathKinds(paths);
        paths.forEach((path, index) => settle(path, kinds[index] ?? "missing"));
    } catch {
        // A lookup that failed is indistinguishable from a file that is not there.
        for (const path of paths) settle(path, "missing");
    }
    if (waiting.size > 0 && flush === null) flush = setTimeout(() => void runBatch(), BATCH_MS);
}

function cached(path: string): PathState | null {
    const held = known.get(path);
    if (!held) return null;
    if (held.state === "missing" && Date.now() - held.at > MISS_TTL_MS) return null;
    return held.state;
}

function ask(path: string, callback: (state: PathState) => void): () => void {
    const callbacks = waiting.get(path) ?? new Set();
    callbacks.add(callback);
    waiting.set(path, callbacks);
    if (flush === null) flush = setTimeout(() => void runBatch(), BATCH_MS);
    return () => {
        callbacks.delete(callback);
        if (callbacks.size === 0 && waiting.get(path) === callbacks) waiting.delete(path);
    };
}

/** Forgets what is known about a path, or about all of them, so the next reference looks again. */
export function forgetPathState(path?: string): void {
    if (path === undefined) known.clear();
    else known.delete(path);
}

/**
 * What a path is, or null until the answer comes back. Lookups are batched
 * across every reference on screen and the answer is kept.
 */
export function usePathState(path: string | null): PathState | null {
    const remote = useRemoteOrigin() !== null;
    const [state, setState] = useState<PathState | null>(() => (path && !remote ? cached(path) : null));
    useEffect(() => {
        if (!path || remote) {
            setState(null);
            return;
        }
        const held = cached(path);
        if (held) {
            setState(held);
            return;
        }
        setState(null);
        let live = true;
        const stop = ask(path, (answer) => {
            if (live) setState(answer);
        });
        return () => {
            live = false;
            stop();
        };
    }, [path, remote]);
    // Another computer's files cannot be looked up here, so what names a path is taken to be one.
    if (path && remote) return path.endsWith("/") ? "dir" : "file";
    return state;
}
