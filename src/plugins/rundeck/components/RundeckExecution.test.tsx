import { act, render } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { LogTick } from "../api";

const { watchStart, watchStop, logsStart, logsStop } = vi.hoisted(() => ({
    watchStart: vi.fn(),
    watchStop: vi.fn(() => Promise.resolve()),
    logsStart: vi.fn(),
    logsStop: vi.fn(() => Promise.resolve()),
}));
vi.mock("../api", () => ({
    rundeckApi: { watchStart, watchStop, logsStart, logsStop, abort: vi.fn(), jobDetail: () => new Promise(() => {}) },
    errorMessage: (e: unknown) => String(e),
}));
vi.mock("../../../plugin-api/ui", async (importOriginal) => ({ ...(await importOriginal<object>()), VirtualLogList: () => null }));

import { RundeckExecution } from "./RundeckExecution";
import { appendLogTick } from "./useExecutionStreams";

const level = { kind: "execution" as const, executionId: 42, project: "ops", jobId: "job-1", name: "api", group: null };

function deferred<T>() {
    let resolve!: (value: T) => void;
    const promise = new Promise<T>((res) => {
        resolve = res;
    });
    return { promise, resolve };
}

function setHidden(hidden: boolean) {
    Object.defineProperty(document, "hidden", { configurable: true, get: () => hidden });
    document.dispatchEvent(new Event("visibilitychange"));
}

afterEach(() => {
    Object.defineProperty(document, "hidden", { configurable: true, get: () => false });
});

describe("RundeckExecution", () => {
    it("stops subscriptions whose ids arrive after unmount", async () => {
        const watch = deferred<number>();
        const logs = deferred<number>();
        watchStart.mockReset().mockReturnValue(watch.promise);
        logsStart.mockReset().mockReturnValue(logs.promise);
        watchStop.mockClear();
        logsStop.mockClear();

        const { unmount } = render(<RundeckExecution paneId="pane" active level={level} />);
        unmount();
        await act(async () => {
            watch.resolve(7);
            logs.resolve(8);
        });

        expect(watchStop).toHaveBeenCalledWith(7);
        expect(logsStop).toHaveBeenCalledWith(8);
    });

    it("resumes the log from the last offset when the window is shown again", async () => {
        watchStart.mockReset().mockResolvedValue(1);
        logsStart.mockReset().mockResolvedValue(2);
        logsStop.mockClear();

        const { unmount } = render(<RundeckExecution paneId="pane" active level={level} />);
        await act(async () => {});
        const onTick = logsStart.mock.calls[0][3] as (tick: LogTick) => void;
        expect(logsStart.mock.calls[0][1]).toBeNull();
        act(() => onTick({ entries: [], offset: "1234", completed: false, failed: false, error: null }));

        await act(async () => setHidden(true));
        expect(logsStop).toHaveBeenCalledWith(2);
        await act(async () => setHidden(false));
        expect(logsStart).toHaveBeenCalledTimes(2);
        expect(logsStart.mock.calls[1][1]).toBe("1234");
        unmount();
    });
});

describe("appendLogTick", () => {
    const entry = (log: string) => ({ time: null, level: null, log, user: null, stepctx: null, node: null });

    it("numbers lines on arrival and counts what it trims", () => {
        const empty = { rows: [], dropped: 0, completed: false, failed: false, error: null };
        const first = appendLogTick(empty, { entries: [entry("a"), entry("b")], offset: "2", completed: false, failed: false, error: null }, 0);
        expect(first.rows.map((row) => row.seq)).toEqual([0, 1]);

        const big = Array.from({ length: 10_000 }, (_, i) => entry(String(i)));
        const next = appendLogTick(first, { entries: big, offset: "3", completed: true, failed: false, error: null }, 2);
        expect(next.rows).toHaveLength(10_000);
        expect(next.dropped).toBe(2);
        expect(next.rows[0].seq).toBe(2);
        expect(next.completed).toBe(true);
    });
});
