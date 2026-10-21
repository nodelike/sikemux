import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Workspace } from "../api/generated/wire";
import { installIpcTransportForTests, MemoryIpcTransport, resetIpcTransportForTests } from "../api/transport";
import { startCoreWorkspace, WORKSPACE_CHANGED_EVENT } from "../state/coreWorkspace";
import { getState, setState } from "../state/store";
import { FOCUS_AGENT_EVENT, RemoteChatBridge } from "./RemoteChatBridge";

const initial = getState();
let transport: MemoryIpcTransport;
let stop: () => void;

const workspace = (agents: Workspace["agents"] = []): Workspace => ({
    projects: [{ id: "p-site", name: "site", path: "/Users/me/site", pinned: false }],
    spaces: [],
    agents,
    pinned: [],
    permissionMode: "bypass",
    disabledPlugins: [],
    launchers: [],
    palette: {},
    backdrop: { texture: false, image: null },
});

const phoneChat: Workspace["agents"][number] = {
    id: "agent-from-phone",
    provider: "codex",
    project: "p-site",
    cwd: "/Users/me/site",
    title: "codex",
    renamed: false,
    permissionMode: "workspace-write",
    keepAlive: false,
    asleep: false,
    createdAt: 1,
};

beforeEach(() => {
    vi.useFakeTimers();
    setState(initial, true);
    transport = new MemoryIpcTransport();
    installIpcTransportForTests(transport);
    transport.register("core_call", () => null);
    stop = startCoreWorkspace(workspace());
});

afterEach(() => {
    stop();
    cleanup();
    resetIpcTransportForTests();
    vi.useRealTimers();
});

async function phoneStartsChat() {
    await act(async () => {
        await vi.advanceTimersByTimeAsync(100);
        transport.emit(WORKSPACE_CHANGED_EVENT, workspace([phoneChat]));
        transport.emit(WORKSPACE_CHANGED_EVENT, workspace([phoneChat]));
        await vi.advanceTimersByTimeAsync(100);
    });
}

describe("RemoteChatBridge", () => {
    it("adds a chat a phone starts to the host without taking the screen, once", async () => {
        const before = getState().activeSessionId;
        await phoneStartsChat();
        const state = getState();
        expect(state.agents["agent-from-phone"]).toMatchObject({ type: "codex", cwd: "/Users/me/site", permissionMode: "workspace-write" });
        expect(state.activeSessionId).toBe(before);
        const windows = Object.values(state.windows).filter((window) => window.role === "agent" && window.activePaneId === "agent-from-phone");
        expect(windows).toHaveLength(1);
    });

    it("shows the agent the notch asks for, switching project on the way", async () => {
        let waiting: string | null = null;
        transport.register("focus_claim", () => {
            const agentId = waiting;
            waiting = null;
            return agentId;
        });
        const view = render(<RemoteChatBridge />);
        await phoneStartsChat();
        expect(getState().sessions[getState().activeSessionId]?.cwd).not.toBe("/Users/me/site");
        waiting = "agent-from-phone";
        await act(async () => {
            transport.emit(FOCUS_AGENT_EVENT, null);
            await vi.advanceTimersByTimeAsync(0);
        });
        const state = getState();
        const session = state.sessions[state.activeSessionId];
        expect(session?.cwd).toBe("/Users/me/site");
        expect(state.windows[session?.activeWindowId ?? ""]?.activePaneId).toBe("agent-from-phone");
        view.unmount();
    });
});
