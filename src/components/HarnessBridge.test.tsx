import { cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { installIpcTransportForTests, MemoryIpcTransport, resetIpcTransportForTests } from "../api/transport";
import { handleHarnessRequest } from "../harness/service";
import { HarnessBridge } from "./HarnessBridge";

vi.mock("../harness/service", () => ({ handleHarnessRequest: vi.fn(), harnessTasks: { output: vi.fn(), closeProject: vi.fn() } }));
afterEach(() => {
    cleanup();
    resetIpcTransportForTests();
    vi.resetAllMocks();
});

describe("harness bridge", () => {
    it("serves another request while an event wait is pending and removes listeners on unmount", async () => {
        const transport = new MemoryIpcTransport();
        installIpcTransportForTests(transport);
        let resolveWait!: (value: unknown) => void;
        const waiting = new Promise((resolve) => {
            resolveWait = resolve;
        });
        vi.mocked(handleHarnessRequest).mockImplementation(async (request) => (request.method === "events.wait" ? waiting : { project: "/one" }));
        transport.register(
            "harness_claim",
            vi
                .fn()
                .mockReturnValueOnce([
                    { id: "wait", method: "events.wait", project: "/one", params: {} },
                    { id: "inspect", method: "workspace.inspect", project: "/one", params: {} },
                ])
                .mockReturnValue([]),
        );
        const reply = vi.fn();
        transport.register("harness_reply", reply);
        const view = render(<HarnessBridge />);
        await waitFor(() => expect(reply).toHaveBeenCalledWith(expect.objectContaining({ id: "inspect" }), expect.anything()));
        expect(reply).toHaveBeenCalledTimes(1);
        resolveWait({ events: [] });
        await waitFor(() => expect(reply).toHaveBeenCalledTimes(2));
        view.unmount();
        expect(transport.eventListenerCount).toBe(0);
    });
});
