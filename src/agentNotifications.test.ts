import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AgentPresentationState, AgentRuntimeState } from "./state/types";
import { getState, setState } from "./state/store";
import { withAgents } from "./test/agents";

const notification = vi.hoisted(() => {
    const sent = vi.fn();
    const state = { permission: "granted" };
    const invoke = vi.fn(async (command: string, args?: { options: { title: string; body: string } }) => {
        if (command === "plugin:notification|request_permission") return state.permission;
        if (command === "plugin:notification|notify") sent(args?.options);
    });
    const bips = vi.fn();
    const node = () => ({
        frequency: { value: 0 },
        gain: { setValueAtTime: vi.fn(), exponentialRampToValueAtTime: vi.fn() },
        connect: (next: unknown) => next,
        start: bips,
        stop: vi.fn(),
    });
    class AudioContext {
        currentTime = 0;
        destination = {};
        resume = vi.fn(async () => {});
        createOscillator = node;
        createGain = node;
    }
    return { sent, bips, state, invoke, AudioContext };
});
vi.mock("@tauri-apps/api/core", () => ({ invoke: notification.invoke }));
vi.stubGlobal("AudioContext", notification.AudioContext);
const appWindow = vi.hoisted(() => ({
    setBadgeCount: vi.fn(async () => {}),
    requestUserAttention: vi.fn(async () => {}),
}));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => appWindow, UserAttentionType: { Informational: 2 } }));

const { agentsNeedingYou, installAgentNotifications, introduceNotifications, newAgentAlerts } = await import("./agentNotifications");

const initial = getState();

function activity(state: AgentPresentationState): AgentRuntimeState {
    const backendState = state === "done" ? "idle" : state;
    return { state, backendState, unread: state === "done", updatedAt: 0, sequence: 1, source: "acp", confidence: "high", reason: "" };
}

beforeEach(() => {
    setState(initial, true);
    const first = getState().activeSessionId;
    setState((state) => ({
        sessionOrder: [first, "s2"],
        sessions: {
            ...state.sessions,
            [first]: { ...state.sessions[first], name: "api" },
            s2: { ...state.sessions[first], id: "s2", name: "openjob" },
        },
    }));
    setState((state) => withAgents(state, first, [{ id: "a1", type: "claude", title: "Claude", startup: "claude", cwd: "/one" }]));
    setState((state) => withAgents(state, "s2", [{ id: "a2", type: "codex", title: "Codex", startup: "codex", cwd: "/two" }]));
    vi.clearAllMocks();
});

describe("which agents are waiting on you", () => {
    it("counts agents asking for input and agents finished unseen", () => {
        setState({ agentActivity: { a1: activity("blocked"), a2: activity("done") } });
        expect(agentsNeedingYou(getState())).toBe(2);
        setState({ agentActivity: { a1: activity("working"), a2: activity("idle") } });
        expect(agentsNeedingYou(getState())).toBe(0);
    });

    it("alerts once as an agent starts waiting, naming its project", () => {
        const before = getState();
        setState({ agentActivity: { a1: activity("working"), a2: activity("blocked") } });
        const after = getState();
        expect(newAgentAlerts(before, after)).toEqual([{ agentId: "a2", title: "Codex needs your input", body: "openjob" }]);
        expect(newAgentAlerts(after, after)).toEqual([]);
    });

    it("alerts when an agent finishes, even the one on screen", () => {
        setState({ agentActivity: { a1: activity("working") } });
        const before = getState();
        setState({ agentActivity: { a1: activity("idle") } });
        expect(newAgentAlerts(before, getState())).toEqual([{ agentId: "a1", title: "Claude finished", body: "api" }]);
    });

    it("does not call an agent that was never working finished", () => {
        const before = getState();
        setState({ agentActivity: { a1: activity("idle") } });
        expect(newAgentAlerts(before, getState())).toEqual([]);
    });
});

describe("notifying outside the app", () => {
    let focused = false;
    let uninstall: () => void;
    beforeEach(() => {
        focused = false;
        uninstall = installAgentNotifications(() => focused);
    });
    afterEach(() => uninstall());

    it("notifies, bounces the dock and badges it while Sikemux is in the background", async () => {
        setState({ agentActivity: { a2: activity("blocked") } });
        await vi.waitFor(() => expect(notification.sent).toHaveBeenCalledWith({ title: "Codex needs your input", body: "openjob" }));
        expect(notification.bips).toHaveBeenCalledTimes(1);
        expect(appWindow.requestUserAttention).toHaveBeenCalledTimes(1);
        expect(appWindow.setBadgeCount).toHaveBeenLastCalledWith(1);
    });

    it("only badges while Sikemux has focus", () => {
        focused = true;
        setState({ agentActivity: { a2: activity("blocked") } });
        expect(appWindow.setBadgeCount).toHaveBeenLastCalledWith(1);
        expect(appWindow.requestUserAttention).not.toHaveBeenCalled();
        expect(notification.sent).not.toHaveBeenCalled();
    });

    it("clears the badge once nothing is waiting", () => {
        setState({ agentActivity: { a2: activity("blocked") } });
        setState({ agentActivity: { a2: activity("working") } });
        expect(appWindow.setBadgeCount).toHaveBeenLastCalledWith(undefined);
    });

    it("stays quiet when notifications are switched off, but keeps the badge", () => {
        setState({ agentNotifications: false, agentActivity: { a1: activity("working") } });
        setState({ agentActivity: { a1: activity("done") } });
        expect(appWindow.setBadgeCount).toHaveBeenLastCalledWith(1);
        expect(appWindow.requestUserAttention).not.toHaveBeenCalled();
        expect(notification.sent).not.toHaveBeenCalled();
    });

    it("asks macOS for permission and stays quiet when it is refused", async () => {
        notification.state.permission = "denied";
        setState({ agentActivity: { a1: activity("blocked") } });
        await vi.waitFor(() => expect(notification.invoke).toHaveBeenCalledWith("plugin:notification|request_permission"));
        expect(notification.sent).not.toHaveBeenCalled();
        expect(notification.bips).not.toHaveBeenCalled();
        notification.state.permission = "granted";
    });
});

describe("the first launch", () => {
    it("greets once with a bip, so macOS asks for permission up front", async () => {
        introduceNotifications();
        introduceNotifications();
        await vi.waitFor(() => expect(notification.sent).toHaveBeenCalledTimes(1));
        expect(notification.sent.mock.calls[0][0].title).toBe("Sikemux notifications are on");
        expect(notification.bips).toHaveBeenCalledTimes(1);
        expect(getState().notificationsIntroduced).toBe(true);
    });

    it("stays quiet for someone who switched notifications off", () => {
        setState({ agentNotifications: false });
        introduceNotifications();
        expect(notification.sent).not.toHaveBeenCalled();
        expect(notification.bips).not.toHaveBeenCalled();
        expect(getState().notificationsIntroduced).toBe(false);
    });
});
