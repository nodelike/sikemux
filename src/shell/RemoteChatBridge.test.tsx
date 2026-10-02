import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { AcpChat } from "../api/acp";
import { installIpcTransportForTests, MemoryIpcTransport, resetIpcTransportForTests } from "../api/transport";
import { getState, setState } from "../state/store";
import { REMOTE_CHAT_BEGUN_EVENT, RemoteChatBridge } from "./RemoteChatBridge";

const initial = getState();
let transport: MemoryIpcTransport;

beforeEach(() => {
    setState(initial, true);
    transport = new MemoryIpcTransport();
    installIpcTransportForTests(transport);
});

afterEach(() => {
    cleanup();
    resetIpcTransportForTests();
});

function phoneChat(): AcpChat {
    return {
        agentId: "agent-from-phone",
        provider: "codex",
        cwd: "/Users/me/site",
        sessionId: null,
        state: "starting",
        running: false,
        pendingPermissions: [],
        startedBy: "phone-key",
        launcher: "codex",
        permissionMode: "workspace-write",
        model: null,
        effort: null,
    };
}

describe("RemoteChatBridge", () => {
    it("adds a chat a phone starts to the Mac without taking the screen, once", async () => {
        const view = render(<RemoteChatBridge />);
        const before = getState().activeSessionId;
        await act(async () => {
            await Promise.resolve();
        });
        act(() => {
            transport.emit(REMOTE_CHAT_BEGUN_EVENT, phoneChat());
            transport.emit(REMOTE_CHAT_BEGUN_EVENT, phoneChat());
        });
        const state = getState();
        expect(state.agents["agent-from-phone"]).toMatchObject({ type: "codex", cwd: "/Users/me/site", permissionMode: "workspace-write" });
        expect(state.activeSessionId).toBe(before);
        const windows = Object.values(state.windows).filter((window) => window.role === "agent" && window.activePaneId === "agent-from-phone");
        expect(windows).toHaveLength(1);
        view.unmount();
        expect(transport.eventListenerCount).toBe(0);
    });
});
