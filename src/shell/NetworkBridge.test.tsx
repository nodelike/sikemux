import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { installIpcTransportForTests, MemoryIpcTransport, resetIpcTransportForTests } from "../api/transport";
import { NETWORK_CHANGED_EVENT, NetworkBridge } from "./NetworkBridge";

const refetchFailed = vi.hoisted(() => vi.fn());
vi.mock("../state/resources", () => ({ refetchFailed }));

let transport: MemoryIpcTransport;

beforeEach(() => {
    refetchFailed.mockReset();
    transport = new MemoryIpcTransport();
    installIpcTransportForTests(transport);
});

afterEach(() => {
    cleanup();
    resetIpcTransportForTests();
});

describe("NetworkBridge", () => {
    it("fetches failed things again when the network comes back, changes or the computer wakes, not when it goes", async () => {
        render(<NetworkBridge />);
        await act(async () => {});

        await act(async () => transport.emit(NETWORK_CHANGED_EVENT, "offline"));
        expect(refetchFailed).not.toHaveBeenCalled();
        for (const change of ["online", "changed", "woke"]) {
            await act(async () => transport.emit(NETWORK_CHANGED_EVENT, change));
        }
        expect(refetchFailed).toHaveBeenCalledTimes(3);
    });
});
