import { act, cleanup, render, waitFor } from "@testing-library/react";
import { createElement } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Options } from "@tauri-apps/plugin-notification";
import * as cmd from "../state/commands";
import { getState, setState } from "../state/store";
import type { AgentRuntimeState } from "../state/types";
import { useToasts } from "../state/toast";
import { AgentNotifications, notificationStateForTransition, sendAgentSystemNotification } from "./AgentNotifications";

const { show, unminimize, setFocus, isFocused, isPermissionGranted, onAction } = vi.hoisted(() => ({
    show: vi.fn().mockResolvedValue(undefined),
    unminimize: vi.fn().mockResolvedValue(undefined),
    setFocus: vi.fn().mockResolvedValue(undefined),
    isFocused: vi.fn().mockResolvedValue(true),
    isPermissionGranted: vi.fn().mockResolvedValue(false),
    onAction: vi.fn().mockResolvedValue({ unregister: vi.fn() }),
}));

vi.mock("@tauri-apps/api/window", () => ({
    getCurrentWindow: () => ({ show, unminimize, setFocus, isFocused }),
}));
vi.mock("@tauri-apps/plugin-notification", () => ({
    isPermissionGranted,
    onAction,
    requestPermission: vi.fn().mockResolvedValue("denied"),
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
    useToasts.setState({ toasts: [] });
});

afterEach(() => {
    cleanup();
    vi.useRealTimers();
});

describe("agent desktop notifications", () => {
    it("recognizes visible working-to-idle transitions as completed work", () => {
        const runtime = (
            state: AgentRuntimeState["state"],
            backendState: AgentRuntimeState["backendState"],
            sequence: number,
        ): AgentRuntimeState => ({
            state,
            backendState,
            unread: false,
            updatedAt: sequence,
            sequence,
            source: "screen",
            confidence: "high",
            reason: "fixture",
        });
        const working = runtime("working", "working", 1);
        const visibleIdle = runtime("idle", "idle", 2);
        const blocked = runtime("blocked", "blocked", 3);

        expect(notificationStateForTransition(visibleIdle, working)).toBe("done");
        expect(notificationStateForTransition(blocked, visibleIdle)).toBe("blocked");
        expect(notificationStateForTransition(blocked, blocked)).toBeNull();
    });

    it("delivers an in-app completion while the agent and app remain visible", async () => {
        vi.useFakeTimers();
        const state = getState();
        const sessionId = state.activeSessionId;
        const agentId = "agent-visible";
        setState({
            sessions: {
                ...state.sessions,
                [sessionId]: {
                    ...state.sessions[sessionId],
                    kind: "project",
                    activeAgentId: agentId,
                    view: "agent",
                },
            },
            agents: {
                [agentId]: { id: agentId, type: "codex", title: "Explain this codebase", startup: "codex" },
            },
            agentsBySession: { ...state.agentsBySession, [sessionId]: [agentId] },
            notificationPreferences: { ...state.notificationPreferences, enabled: true, delayMs: 0, sounds: false },
        });
        render(createElement(AgentNotifications));

        act(() => {
            cmd.noteAgentActivity(agentId, {
                agentId,
                state: "working",
                sequence: 1,
                source: "activity",
                confidence: "high",
                reason: "command submitted",
            });
            cmd.noteAgentActivity(agentId, {
                agentId,
                state: "idle",
                sequence: 2,
                source: "screen",
                confidence: "high",
                reason: "prompt visible",
            });
        });
        await act(() => vi.runAllTimersAsync());

        expect(getState().agentActivity[agentId]).toMatchObject({ state: "idle", backendState: "idle" });
        expect(useToasts.getState().toasts.at(-1)?.text).toBe("Explain this codebase is done");
        expect(isPermissionGranted).not.toHaveBeenCalled();
    });

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
