import { cleanup, render, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { installIpcTransportForTests, MemoryIpcTransport, resetIpcTransportForTests } from "../api/transport";
import { handleHarnessRequest } from "../harness/service";
import { HarnessBridge } from "./HarnessBridge";
import { closeSession, createProjectSession } from "../state/commands";
import { useStore } from "../state/store";

vi.mock("../harness/service", () => ({ handleHarnessRequest: vi.fn() }));
afterEach(() => {
    cleanup();
    resetIpcTransportForTests();
    vi.resetAllMocks();
});

describe("harness bridge", () => {
    it("serves another request while a slow one is pending and removes listeners on unmount", async () => {
        const transport = new MemoryIpcTransport();
        installIpcTransportForTests(transport);
        let resolveWait!: (value: unknown) => void;
        const waiting = new Promise((resolve) => {
            resolveWait = resolve;
        });
        vi.mocked(handleHarnessRequest).mockImplementation(async (request) => (request.method === "task.start" ? waiting : { project: "/one" }));
        transport.register(
            "harness_claim",
            vi
                .fn()
                .mockReturnValueOnce([
                    { id: "start", method: "task.start", project: "/one", params: {} },
                    { id: "inspect", method: "workspace.inspect", project: "/one", params: {} },
                ])
                .mockReturnValue([]),
        );
        const reply = vi.fn();
        transport.register("harness_reply", reply);
        const view = render(<HarnessBridge />);
        await waitFor(() => expect(reply).toHaveBeenCalledWith(expect.objectContaining({ id: "inspect" }), expect.anything()));
        expect(reply).toHaveBeenCalledTimes(1);
        resolveWait({});
        await waitFor(() => expect(reply).toHaveBeenCalledTimes(2));
        view.unmount();
        expect(transport.eventListenerCount).toBe(0);
    });
});

describe("closing a project", () => {
    it("asks the core to stop that project's harness tasks", async () => {
        const transport = new MemoryIpcTransport();
        installIpcTransportForTests(transport);
        transport.register("harness_claim", () => []);
        const stop = vi.fn();
        transport.register("harness_stop_runs", stop);
        createProjectSession("/gone");
        const view = render(<HarnessBridge />);
        const session = Object.values(useStore.getState().sessions).find((session) => session.cwd === "/gone")!;
        closeSession(session.id);
        await waitFor(() => expect(stop).toHaveBeenCalledWith({ project: "/gone" }, expect.anything()));
        view.unmount();
    });
});
