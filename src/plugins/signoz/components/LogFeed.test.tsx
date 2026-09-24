import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { LogLine, TailTick } from "../api";

const { tailStart, tailStop, searchLogs, logVolume } = vi.hoisted(() => ({
    tailStart: vi.fn(),
    tailStop: vi.fn(() => Promise.resolve()),
    searchLogs: vi.fn(),
    logVolume: vi.fn(() => Promise.resolve([])),
}));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), signozApi: { tailStart, tailStop, searchLogs, logVolume } }));
vi.mock("../../../plugin-api/ui", async (importOriginal) => ({
    ...(await importOriginal<object>()),
    VirtualLogList: ({ items, renderRow }: { items: unknown[]; renderRow: (item: unknown, index: number) => unknown }) => (
        <div>{items.map((item, index) => renderRow(item, index) as never)}</div>
    ),
}));

import { setLive } from "../state";
import { LogFeed } from "./LogFeed";

afterEach(cleanup);

const line: LogLine = {
    id: "l1",
    timestamp: "2026-09-24T09:11:54.569Z",
    service: "api-gateway",
    severity: "ERROR",
    body: "Request completed with server error",
    traceId: null,
    spanId: null,
    attributes: { path: "/upload" },
    resources: {},
};

describe("LogFeed", () => {
    it("stops a tail whose id arrives after the pane has gone", async () => {
        let resolveId!: (id: number) => void;
        tailStart.mockReturnValue(new Promise<number>((resolve) => (resolveId = resolve)));
        const { unmount } = render(<LogFeed paneId="pane-late" active />);
        unmount();
        await act(async () => resolveId(9));
        expect(tailStop).toHaveBeenCalledWith(9);
    });

    it("asks for errors only until told otherwise", () => {
        tailStart.mockReset().mockReturnValue(new Promise<number>(() => {}));
        render(<LogFeed paneId="pane-errors" active />);
        expect(tailStart.mock.calls[0][0]).toMatchObject({ severities: ["FATAL", "ERROR"], minutes: 15 });
    });

    it("reads a held window page by page instead of tailing it", async () => {
        tailStart.mockReset();
        searchLogs.mockReset().mockResolvedValue({ lines: [line], nextOffset: 200 });
        setLive("pane-held", false);
        await act(async () => {
            render(<LogFeed paneId="pane-held" active />);
        });
        expect(tailStart).not.toHaveBeenCalled();
        expect(searchLogs.mock.calls[0][0]).toMatchObject({ offset: 0, limit: 200 });
        expect(searchLogs.mock.calls[0][0].start).toBeTypeOf("number");
        await act(async () => {
            fireEvent.click(screen.getByText("Load older lines"));
        });
        expect(searchLogs.mock.calls[1][0]).toMatchObject({ offset: 200 });
    });

    it("narrows the feed to an attribute's value from the line itself", async () => {
        let deliver!: (tick: TailTick) => void;
        tailStart.mockReset().mockImplementation((_search: unknown, onTick: (tick: TailTick) => void) => {
            deliver = onTick;
            return new Promise<number>(() => {});
        });
        render(<LogFeed paneId="pane-narrow" active />);
        await act(async () => deliver({ lines: [line], error: null }));
        fireEvent.click(screen.getByText("Request completed with server error"));
        fireEvent.click(screen.getByTitle("Only lines where path is this"));
        const restarted = tailStart.mock.calls.at(-1)![0];
        expect(restarted.filters).toEqual([{ key: "path", op: "equals", value: "/upload" }]);
    });
});
