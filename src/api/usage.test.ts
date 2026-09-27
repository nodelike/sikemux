import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { getState, setState } from "../state/store";
import { MemoryIpcTransport, installIpcTransportForTests, resetIpcTransportForTests } from "./transport";
import { reportActive } from "./usage";

const initial = getState();
let transport: MemoryIpcTransport;

beforeEach(() => {
    resetIpcTransportForTests();
    transport = new MemoryIpcTransport();
    installIpcTransportForTests(transport);
    setState(initial, true);
});

afterEach(() => {
    resetIpcTransportForTests();
    setState(initial, true);
});

describe("usage report", () => {
    it("reports the update channel the person follows", async () => {
        const handler = vi.fn(() => Promise.resolve(null));
        transport.register("usage_report_active", handler);
        setState({ updateChannel: "nightly" });

        await reportActive();

        expect(handler).toHaveBeenCalledWith({ channel: "nightly" }, expect.anything());
    });

    it("sends nothing once the person opts out", async () => {
        const handler = vi.fn(() => Promise.resolve(null));
        transport.register("usage_report_active", handler);
        setState({ shareUsageData: false });

        await reportActive();

        expect(handler).not.toHaveBeenCalled();
    });

    it("keeps a failed report quiet", async () => {
        transport.register("usage_report_active", () => Promise.reject(new Error("usage report: offline")));

        await expect(reportActive()).resolves.toBeUndefined();
    });
});
