import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { ContainerDetail, LogBatch } from "../api";
import type { PluginStreamHandlers } from "../../../plugin-api/backend";

const streams = vi.hoisted(() => ({ handlers: [] as PluginStreamHandlers<LogBatch>[], stopped: 0 }));
const api = vi.hoisted(() => ({
    followLogs: vi.fn((_id: string, handlers: PluginStreamHandlers<LogBatch>) => {
        streams.handlers.push(handlers);
        return { stop: () => (streams.stopped += 1) };
    }),
}));
vi.mock("../api", async (importOriginal) => ({ ...(await importOriginal<object>()), dockerApi: api }));
const copyWithNotice = vi.hoisted(() => vi.fn());
vi.mock("../../../plugin-api/host", async (importOriginal) => ({ ...(await importOriginal<object>()), copyWithNotice }));

import { LogView } from "./LogView";

const web = {
    id: "bbbbbbbbbbbb",
    name: "shop-web-1",
    state: {
        status: "running",
        running: true,
        exitCode: 0,
        error: null,
        oomKilled: false,
        startedAt: "2026-10-18T10:00:00Z",
        finishedAt: null,
        health: null,
    },
} as ContainerDetail;

const latest = () => streams.handlers[streams.handlers.length - 1]!;
const send = (batch: LogBatch) => act(() => latest().onItem(batch));

beforeEach(() => {
    streams.handlers = [];
    streams.stopped = 0;
    api.followLogs.mockClear();
    copyWithNotice.mockClear();
});
afterEach(cleanup);

describe("a container's log", () => {
    it("follows the container's output as it arrives, and stops when the view goes", () => {
        const { unmount } = render(<LogView container={web} />);
        expect(api.followLogs).toHaveBeenCalledWith("bbbbbbbbbbbb", expect.anything());
        expect(screen.getByText("Waiting for output…")).toBeInTheDocument();
        send({ lines: [{ at: null, stream: "stdout", text: "listening on :3000" }], ended: false });
        send({ lines: [{ at: null, stream: "stderr", text: "ERROR db refused" }], ended: false });
        expect(screen.getByRole("status")).toHaveTextContent("2 lines");
        unmount();
        expect(streams.stopped).toBe(1);
    });

    it("narrows to stderr or to words, and copies only what it shows", () => {
        render(<LogView container={web} />);
        send({
            lines: [
                { at: null, stream: "stdout", text: "GET / 200" },
                { at: null, stream: "stderr", text: "ERROR db refused" },
            ],
            ended: false,
        });
        fireEvent.click(screen.getByRole("button", { name: "Errors only" }));
        expect(screen.getByRole("status")).toHaveTextContent("1 of 2 lines");
        fireEvent.click(screen.getByRole("button", { name: "Copy" }));
        expect(copyWithNotice).toHaveBeenCalledWith("ERROR db refused", "Matching log lines");
        fireEvent.click(screen.getByRole("button", { name: "Errors only" }));
        fireEvent.change(screen.getByRole("textbox", { name: "Search the log" }), { target: { value: "nothing" } });
        expect(screen.getByText("No lines match.")).toBeInTheDocument();
    });

    it("follows again when the container starts again", () => {
        const { rerender } = render(<LogView container={{ ...web, state: { ...web.state, running: false } }} />);
        send({ lines: [], ended: true });
        expect(screen.getByText(/The log ended when the container stopped/)).toBeInTheDocument();
        rerender(<LogView container={{ ...web, state: { ...web.state, startedAt: "2026-10-18T11:00:00Z" } }} />);
        expect(api.followLogs).toHaveBeenCalledTimes(2);
        expect(streams.stopped).toBe(1);
    });
});
