import { waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Options } from "@tauri-apps/plugin-notification";
import { getState, setState } from "../state/store";
import { sendAgentSystemNotification } from "./AgentNotifications";

const { show, unminimize, setFocus } = vi.hoisted(() => ({
    show: vi.fn().mockResolvedValue(undefined),
    unminimize: vi.fn().mockResolvedValue(undefined),
    setFocus: vi.fn().mockResolvedValue(undefined),
}));

vi.mock("@tauri-apps/api/window", () => ({
    getCurrentWindow: () => ({ show, unminimize, setFocus }),
}));

class FakeNotification {
    static latest: FakeNotification | null = null;
    onclick: ((event: Event) => void) | null = null;
    close = vi.fn();

    constructor(
        readonly title: string,
        readonly options?: NotificationOptions,
    ) {
        FakeNotification.latest = this;
    }
}

const initial = getState();

beforeEach(() => {
    setState(initial, true);
    show.mockClear();
    unminimize.mockClear();
    setFocus.mockClear();
    FakeNotification.latest = null;
    vi.stubGlobal("Notification", FakeNotification);
});

describe("agent desktop notifications", () => {
    it("focuses the exact session and agent when the native notification is clicked", async () => {
        const state = getState();
        const currentSessionId = state.activeSessionId;
        const targetSessionId = "project-target";
        const agentId = "agent-target";
        setState({
            sessions: {
                ...state.sessions,
                [targetSessionId]: {
                    id: targetSessionId,
                    name: "target",
                    kind: "project",
                    cwd: "/repo/target",
                    pinned: false,
                    activeWindowId: "",
                    activeAgentId: null,
                    view: "windows",
                },
            },
            sessionOrder: [currentSessionId, targetSessionId],
            windowsBySession: { ...state.windowsBySession, [targetSessionId]: [] },
            agents: {
                [agentId]: {
                    id: agentId,
                    type: "codex",
                    title: "target agent",
                    startup: "codex resume exact",
                    resumeId: "exact",
                },
            },
            agentsBySession: { ...state.agentsBySession, [targetSessionId]: [agentId] },
        });
        const options: Options = { title: "Agent finished", body: "target agent", extra: { targetSessionId, agentId } };

        sendAgentSystemNotification(options, targetSessionId, agentId);
        FakeNotification.latest?.onclick?.(new Event("click"));

        await waitFor(() => {
            expect(getState().activeSessionId).toBe(targetSessionId);
            expect(getState().sessions[targetSessionId]).toMatchObject({ view: "agent", activeAgentId: agentId });
            expect(setFocus).toHaveBeenCalledOnce();
        });
        expect(FakeNotification.latest?.close).toHaveBeenCalledOnce();
    });
});
