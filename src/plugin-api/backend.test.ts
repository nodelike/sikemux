import { beforeEach, describe, expect, it, vi } from "vitest";
import type { PluginStreamEvent } from "../api/plugins";
import { createPluginBackend, isPluginFailure } from "./backend";

const { pluginsApi } = vi.hoisted(() => ({
    pluginsApi: { call: vi.fn(), streamStart: vi.fn(), streamStop: vi.fn() },
}));

vi.mock("../api/plugins", () => ({ pluginsApi }));

function startedStream() {
    let emit!: (event: PluginStreamEvent) => void;
    let resolveId!: (id: number) => void;
    pluginsApi.streamStart.mockImplementation((_plugin: string, _method: string, _params: unknown, onEvent: (event: PluginStreamEvent) => void) => {
        emit = onEvent;
        return new Promise<number>((resolve) => {
            resolveId = resolve;
        });
    });
    return { emit: (event: PluginStreamEvent) => emit(event), resolveId: (id: number) => resolveId(id) };
}

beforeEach(() => {
    pluginsApi.call.mockReset();
    pluginsApi.streamStart.mockReset();
    pluginsApi.streamStop.mockReset().mockResolvedValue(undefined);
});

describe("createPluginBackend", () => {
    it("calls methods on its own plugin", async () => {
        pluginsApi.call.mockResolvedValue({ ok: true });
        await expect(createPluginBackend("sikemux.rundeck").call("status")).resolves.toEqual({ ok: true });
        expect(pluginsApi.call).toHaveBeenCalledWith("sikemux.rundeck", "status", null);
    });

    it("delivers items until the stream ends", async () => {
        const native = startedStream();
        const onItem = vi.fn();
        const onEnd = vi.fn();
        createPluginBackend("sikemux.rundeck").stream("logs", { id: 1 }, { onItem, onEnd });
        native.emit({ kind: "item", value: "a" });
        native.emit({ kind: "end" });
        native.emit({ kind: "item", value: "late" });
        expect(onItem.mock.calls).toEqual([["a"]]);
        expect(onEnd).toHaveBeenCalledOnce();
    });

    it("stops a stream that was cancelled before it finished starting", async () => {
        const native = startedStream();
        const onItem = vi.fn();
        const stream = createPluginBackend("sikemux.rundeck").stream("logs", null, { onItem });
        stream.stop();
        native.resolveId(7);
        await vi.waitFor(() => expect(pluginsApi.streamStop).toHaveBeenCalledWith(7));
        native.emit({ kind: "item", value: "after stop" });
        expect(onItem).not.toHaveBeenCalled();
    });

    it("reports a stream that fails to start", async () => {
        pluginsApi.streamStart.mockRejectedValue({ category: "not-installed", message: "no plugin" });
        const onError = vi.fn();
        createPluginBackend("sikemux.gone").stream("logs", null, { onItem: vi.fn(), onError });
        await vi.waitFor(() => expect(onError).toHaveBeenCalledWith({ category: "not-installed", message: "no plugin" }));
    });
});

describe("isPluginFailure", () => {
    it("matches the wire shape and an optional category", () => {
        const failure = { category: "unconfigured", message: "sign in", plugin: "sikemux.rundeck" };
        expect(isPluginFailure(failure)).toBe(true);
        expect(isPluginFailure(failure, "unconfigured")).toBe(true);
        expect(isPluginFailure(failure, "auth")).toBe(false);
        expect(isPluginFailure(new Error("x"))).toBe(false);
    });
});
