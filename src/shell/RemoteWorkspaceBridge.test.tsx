import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { RemoteStatus } from "../api/remote";
import { installIpcTransportForTests, MemoryIpcTransport, resetIpcTransportForTests } from "../api/transport";
import * as cmd from "../state/commands";
import { resetResourcesForTests } from "../state/resources";
import { getState, setState } from "../state/store";
import { PUBLISH_DELAY_MS, RemoteWorkspaceBridge } from "./RemoteWorkspaceBridge";

const initial = getState();

function status(enabled: boolean): RemoteStatus {
    return {
        enabled,
        coreId: "core",
        addresses: [],
        devices: [],
        connected: [],
        pending: [],
        owner: null,
        account: null,
        updateRequired: null,
        notifications: [],
        requireApproval: false,
    };
}

let transport: MemoryIpcTransport;

beforeEach(() => {
    vi.useFakeTimers();
    setState(initial, true);
    transport = new MemoryIpcTransport();
    installIpcTransportForTests(transport);
});

afterEach(() => {
    cleanup();
    resetResourcesForTests();
    resetIpcTransportForTests();
    vi.useRealTimers();
});

async function settle() {
    await act(async () => {
        await vi.advanceTimersByTimeAsync(PUBLISH_DELAY_MS + 50);
    });
}

describe("RemoteWorkspaceBridge", () => {
    it("publishes which agent is on screen", async () => {
        transport.registerCoreRequest("remoteStatus", () => ({ status: status(false) }));
        const onScreen = vi.fn<(args: unknown) => void>();
        transport.register("remote_publish", (args) => {
            const { request } = args as { request: { op: string } };
            if (request.op === "publishOnScreen") onScreen(request);
        });
        render(<RemoteWorkspaceBridge />);
        await settle();
        act(() => {
            cmd.createProjectSession("/Users/me/notch");
            cmd.addAgent("claude", undefined, "Fix the login flake");
        });
        await settle();
        const agentId = Object.values(getState().agents).find((agent) => agent.title === "Fix the login flake")?.id ?? "";
        expect(onScreen.mock.lastCall?.[0]).toMatchObject({ agentIds: [agentId] });
    });

    it("publishes the agent on screen as unwatched while the window is behind another app", async () => {
        transport.registerCoreRequest("remoteStatus", () => ({ status: status(false) }));
        const onScreen = vi.fn<(args: unknown) => void>();
        transport.register("remote_publish", (args) => {
            const { request } = args as { request: { op: string } };
            if (request.op === "publishOnScreen") onScreen(request);
        });
        render(<RemoteWorkspaceBridge />);
        act(() => {
            cmd.createProjectSession("/Users/me/notch");
            cmd.addAgent("claude", undefined, "Watch the build");
        });
        await settle();
        const agentId = Object.values(getState().agents).find((agent) => agent.title === "Watch the build")?.id ?? "";
        act(() => {
            window.dispatchEvent(new Event("blur"));
        });
        await settle();
        expect(onScreen.mock.lastCall?.[0]).toEqual({ op: "publishOnScreen", agentIds: [agentId], focused: false });
        act(() => {
            window.dispatchEvent(new Event("focus"));
        });
        await settle();
        expect(onScreen.mock.lastCall?.[0]).toEqual({ op: "publishOnScreen", agentIds: [agentId], focused: true });
    });
});
