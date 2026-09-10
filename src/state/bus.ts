import { performanceTelemetry } from "../lib/performance";

export type Event =
    | { type: "open-file"; path: string; line?: number; character?: number }
    | { type: "close-file"; paneId: string; path: string }
    | { type: "fs-changed"; repo: string }
    | { type: "path-renamed"; src: string; dest: string }
    | { type: "tree-native-drag-hover"; cwd: string | null; targetDir: string | null; highlightPath: string | null }
    | { type: "git-refresh"; repo: string }
    | { type: "agent-focus"; sessionId: string }
    | { type: "search-focus"; sessionId: string }
    | { type: "rnd-auth-expired"; reason: string }
    | { type: "aws-auth-expired"; profile: string; reason: string }
    | { type: "bruno-run"; sessionId: string };

type AnyHandler = (e: Event) => void;

const handlers = new Map<Event["type"], Set<AnyHandler>>();

export function subscribe<T extends Event["type"]>(type: T, fn: (e: Extract<Event, { type: T }>) => void): () => void {
    let set = handlers.get(type);
    if (!set) {
        set = new Set();
        handlers.set(type, set);
    }
    set.add(fn as AnyHandler);
    return () => {
        set!.delete(fn as AnyHandler);
        if (set!.size === 0) handlers.delete(type);
    };
}

export function emit<T extends Event["type"]>(e: Extract<Event, { type: T }>): void {
    const listeners = handlers.get(e.type);
    if (!listeners) return;
    const span = performanceTelemetry.startTrace("bus.emit", { event: e.type, handlers: listeners.size });
    try {
        listeners.forEach((fn) => fn(e));
        const recorded = performanceTelemetry.endSpan(span, { outcome: "success" });
        if (recorded) performanceTelemetry.recordLatency(`bus.${e.type}`, recorded.durationMs);
    } catch (error) {
        const recorded = performanceTelemetry.endSpan(span, { outcome: "error" });
        if (recorded) performanceTelemetry.recordLatency(`bus.${e.type}`, recorded.durationMs);
        throw error;
    }
}

export function busStats(): { eventTypes: number; handlers: number } {
    let count = 0;
    for (const set of handlers.values()) count += set.size;
    return { eventTypes: handlers.size, handlers: count };
}
