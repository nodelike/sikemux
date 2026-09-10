import { describe, expect, it, vi, afterEach } from "vitest";
import { HarnessEvents } from "./events";

afterEach(() => vi.useRealTimers());

describe("harness events", () => {
    it("does not miss an event between inspection and waiting", async () => {
        const events = new HarnessEvents();
        const cursor = events.cursor;
        events.publish({ project: "/one", kind: "task.completed", executionId: "run" });
        const result = await events.wait("/one", cursor, 30_000, "run");
        expect(result.events).toHaveLength(1);
        expect((await events.wait("/one", result.cursor, 0)).events).toEqual([]);
    });
    it("waits for matching project and execution, then cleans up", async () => {
        const events = new HarnessEvents();
        const done = vi.fn();
        const wait = events.wait("/one", events.cursor, 30_000, "run").then(done);
        events.publish({ project: "/two", kind: "task.output", executionId: "run" });
        events.publish({ project: "/one", kind: "task.output", executionId: "other" });
        await Promise.resolve();
        expect(done).not.toHaveBeenCalled();
        events.publish({ project: "/one", kind: "task.output", executionId: "run" });
        await wait;
        expect(done.mock.calls[0][0].events).toHaveLength(1);
    });
    it("bounds waiting, detects expired cursors, and supports cancellation", async () => {
        vi.useFakeTimers();
        const events = new HarnessEvents();
        const cursor = events.cursor;
        const waiting = events.wait("/one", cursor, 100);
        await vi.advanceTimersByTimeAsync(100);
        expect((await waiting).events).toEqual([]);
        for (let i = 0; i < 257; i++) events.publish({ project: "/one", kind: "task.output" });
        expect((await events.wait("/one", cursor, 0)).truncated).toBe(true);
        await expect(events.wait("/one", new HarnessEvents().cursor, 0)).rejects.toThrow("earlier app session");
        await expect(events.wait("/one", events.cursor, 30001)).rejects.toThrow("timeoutMs");
        const controller = new AbortController();
        const cancelled = events.wait("/one", events.cursor, 100, undefined, controller.signal);
        const assertion = expect(cancelled).rejects.toThrow();
        controller.abort();
        await assertion;
        expect(vi.getTimerCount()).toBe(0);
    });
});
