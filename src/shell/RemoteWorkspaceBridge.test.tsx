import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { REMOTE_STATUS_EVENT, type RemoteStatus } from "../api/remote";
import { installIpcTransportForTests, MemoryIpcTransport, resetIpcTransportForTests } from "../api/transport";
import * as cmd from "../state/commands";
import { getState, setState } from "../state/store";
import { PUBLISH_DELAY_MS, RemoteWorkspaceBridge } from "./RemoteWorkspaceBridge";

const initial = getState();

function status(enabled: boolean): RemoteStatus {
    return { enabled, coreId: "core", addresses: [], devices: [], connected: [], pairing: null, pending: [] };
}

let transport: MemoryIpcTransport;
let publish: ReturnType<typeof vi.fn<(args: unknown) => void>>;

beforeEach(() => {
    vi.useFakeTimers();
    setState(initial, true);
    transport = new MemoryIpcTransport();
    installIpcTransportForTests(transport);
    publish = vi.fn<(args: unknown) => void>();
    transport.register("remote_publish_workspace", publish);
});

afterEach(() => {
    cleanup();
    resetIpcTransportForTests();
    vi.useRealTimers();
});

async function settle() {
    await act(async () => {
        await vi.advanceTimersByTimeAsync(PUBLISH_DELAY_MS + 50);
    });
}

describe("RemoteWorkspaceBridge", () => {
    it("publishes nothing while remote access is off", async () => {
        transport.register("remote_status", () => status(false));
        render(<RemoteWorkspaceBridge />);
        await settle();
        expect(publish).not.toHaveBeenCalled();
    });

    it("publishes once remote access turns on, and again when a project opens", async () => {
        transport.register("remote_status", () => status(false));
        render(<RemoteWorkspaceBridge />);
        await settle();
        act(() => {
            transport.emit(REMOTE_STATUS_EVENT, status(true));
        });
        await settle();
        expect(publish).toHaveBeenCalledTimes(1);
        const before = publish.mock.calls[0][0] as { projects: { path: string }[] };

        act(() => {
            cmd.createProjectSession("/Users/me/new-project");
        });
        await settle();
        expect(publish).toHaveBeenCalledTimes(2);
        const after = publish.mock.calls[1][0] as { projects: { path: string }[]; launchers: { id: string }[] };
        expect(after.projects.length).toBe(before.projects.length + 1);
        expect(after.projects.map((project) => project.path)).toContain("/Users/me/new-project");
        expect(after.launchers.map((launcher) => launcher.id)).toContain("opencode");
    });
});
