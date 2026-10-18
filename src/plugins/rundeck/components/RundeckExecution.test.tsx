import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PluginStreamHandlers } from "../../../plugin-api/backend";
import type { LogTick, WatchUpdate } from "../api";

const { watch, logs } = vi.hoisted(() => ({ watch: vi.fn(), logs: vi.fn() }));
vi.mock("../api", () => ({
    rundeckApi: {
        watch,
        logs,
        abort: vi.fn(),
        jobDetail: () => new Promise(() => {}),
        executions: () => new Promise(() => {}),
        branchOptions: async () => ["BRANCH"],
    },
    failureMessage: (e: unknown) => String(e),
}));
vi.mock("../../../plugin-api/ui", async (importOriginal) => ({ ...(await importOriginal<object>()), VirtualLogList: () => null }));

import { RundeckExecution } from "./RundeckExecution";
import { appendLogTick } from "./useExecutionStreams";

const level = { kind: "execution" as const, executionId: 42, project: "ops", jobId: "job-1", name: "api", group: null };

interface Stream<Item> {
    handlers: PluginStreamHandlers<Item>;
    stop: ReturnType<typeof vi.fn>;
}

const opened = <Item,>(mock: ReturnType<typeof vi.fn>): Stream<Item>[] => mock.mock.results.map((result) => result.value as Stream<Item>);

beforeEach(() => {
    watch.mockReset().mockImplementation((_id: number, handlers: PluginStreamHandlers<WatchUpdate>) => ({ handlers, stop: vi.fn() }));
    logs.mockReset().mockImplementation((_id: number, _offset: string | null, handlers: PluginStreamHandlers<LogTick>) => ({
        handlers,
        stop: vi.fn(),
    }));
});

function setHidden(hidden: boolean) {
    Object.defineProperty(document, "hidden", { configurable: true, get: () => hidden });
    document.dispatchEvent(new Event("visibilitychange"));
}

afterEach(() => {
    Object.defineProperty(document, "hidden", { configurable: true, get: () => false });
});

describe("RundeckExecution", () => {
    it("stops both streams when it closes", () => {
        const { unmount } = render(<RundeckExecution paneId="pane" active level={level} />);
        unmount();
        expect(opened(watch)[0].stop).toHaveBeenCalled();
        expect(opened(logs)[0].stop).toHaveBeenCalled();
    });

    it("resumes the log from the last offset when the window is shown again", async () => {
        const { unmount } = render(<RundeckExecution paneId="pane" active level={level} />);
        await act(async () => {});
        expect(logs.mock.calls[0][1]).toBeNull();
        act(() => opened<LogTick>(logs)[0].handlers.onItem({ entries: [], offset: "1234", completed: false, failed: false, error: null }));

        await act(async () => setHidden(true));
        expect(opened(logs)[0].stop).toHaveBeenCalled();
        await act(async () => setHidden(false));
        expect(logs).toHaveBeenCalledTimes(2);
        expect(logs.mock.calls[1][1]).toBe("1234");
        unmount();
    });
});

describe("RundeckExecution notices", () => {
    it("offers to say when a running run ends, and then says it will", async () => {
        const { unmount } = render(<RundeckExecution paneId="pane" active level={level} />);
        act(() =>
            opened<WatchUpdate>(watch)[0].handlers.onItem({
                execution: { id: 42, status: "running", customStatus: null, job: null, project: "ops", permalink: null } as never,
                state: null,
                error: null,
                terminal: false,
            }),
        );
        const button = await screen.findByRole("button", { name: /Notify me/ });
        await act(async () => fireEvent.click(button));
        expect(watch).toHaveBeenCalledTimes(2);
        expect(screen.getByRole("button", { name: /Will notify/ })).toBeDisabled();
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
