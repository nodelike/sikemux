import { cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import { installIpcTransportForTests, MemoryIpcTransport, resetIpcTransportForTests, type MemoryInvokeHandler } from "../api/transport";
import { CliOpenBridge } from "./CliOpenBridge";

let transport: MemoryIpcTransport;
let ready: Mock<MemoryInvokeHandler>;
let claim: Mock<MemoryInvokeHandler>;

beforeEach(() => {
    resetIpcTransportForTests();
    transport = new MemoryIpcTransport();
    installIpcTransportForTests(transport);
    ready = vi.fn<MemoryInvokeHandler>(() => []);
    claim = vi.fn<MemoryInvokeHandler>(() => []);
    transport.register("cli_frontend_ready", ready);
    transport.register("cli_claim_open_requests", claim);
});

afterEach(() => {
    cleanup();
    resetIpcTransportForTests();
});

describe("CliOpenBridge IPC events", () => {
    it("claims after transport registration and aborts delivery on unmount", async () => {
        const view = render(<CliOpenBridge />);
        await waitFor(() => expect(transport.eventListenerCount).toBe(1));
        await waitFor(() => expect(ready).toHaveBeenCalledOnce());

        transport.emit("cli-open-available", null);
        await waitFor(() => expect(claim).toHaveBeenCalledOnce());

        view.unmount();
        expect(transport.eventListenerCount).toBe(0);
        transport.emit("cli-open-available", null);
        expect(claim).toHaveBeenCalledOnce();
    });
});
