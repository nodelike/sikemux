import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { installIpcTransportForTests, MemoryIpcTransport, resetIpcTransportForTests } from "../api/transport";
import type { AgentGroup, AgentShown } from "../api/generated/wire";
import { getState, setState } from "../state/store";
import { withAgents } from "../test/agents";
import { testPreferences } from "../test/preferences";

const notification = vi.hoisted(() => {
    const sent = vi.fn();
    const invoke = vi.fn(async (command: string, args?: { title: string; body: string; agentId?: string }) => {
        if (command === "post_notification") sent(args);
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
    return { sent, bips, invoke, AudioContext };
});
vi.mock("@tauri-apps/api/core", () => ({ invoke: notification.invoke }));
vi.stubGlobal("AudioContext", notification.AudioContext);
const appWindow = vi.hoisted(() => ({
    setBadgeCount: vi.fn(async () => {}),
    requestUserAttention: vi.fn(async () => {}),
}));
vi.mock("@tauri-apps/api/window", () => ({ getCurrentWindow: () => appWindow, UserAttentionType: { Informational: 2 } }));

const { installAgentNotifications, introduceNotifications } = await import("./agentNotifications");

const initial = getState();

function shown(agentId: string, group: AgentGroup): Record<string, AgentShown> {
    const busy = group === "working" || group === "blocked";
    return {
        [agentId]: {
            agentId,
            cli: "codex",
            title: agentId,
            status: group,
            group,
            label: group,
            unread: group === "done",
            subagents: 0,
            tasks: 0,
            busy,
            background: false,
            activeAt: null,
            face: null,
        },
    };
}

const notice = { title: "Codex needs your input", body: "openjob on studio", agentId: "a2" };

beforeEach(() => {
    setState(initial, true);
    setState({ preferences: testPreferences() });
    const first = getState().activeSessionId;
    setState((state) => ({
        sessionOrder: [first, "s2"],
        sessions: {
            ...state.sessions,
            [first]: { ...state.sessions[first], name: "api" },
            s2: { ...state.sessions[first], id: "s2", name: "openjob" },
        },
    }));
    setState((state) => withAgents(state, first, [{ id: "a1", type: "claude", title: "Claude", cwd: "/one" }]));
    setState((state) => withAgents(state, "s2", [{ id: "a2", type: "codex", title: "Codex", cwd: "/two" }]));
    vi.clearAllMocks();
});

describe("notifying outside the app", () => {
    let focused = false;
    let transport: MemoryIpcTransport;
    let uninstall: () => void;
    beforeEach(async () => {
        focused = false;
        transport = new MemoryIpcTransport();
        installIpcTransportForTests(transport);
        uninstall = installAgentNotifications(() => focused);
        await Promise.resolve();
    });
    afterEach(() => {
        uninstall();
        resetIpcTransportForTests();
    });

    it("posts what the core says, with a bip and a bounce, while Sikemux is in the background", async () => {
        transport.emit("agent_notice", notice);
        await vi.waitFor(() => expect(notification.sent).toHaveBeenCalledWith(notice));
        expect(notification.bips).toHaveBeenCalledTimes(1);
        expect(appWindow.requestUserAttention).toHaveBeenCalledTimes(1);
    });

    it("stays quiet while Sikemux has focus, or with notifications switched off", () => {
        focused = true;
        transport.emit("agent_notice", notice);
        focused = false;
        setState({ preferences: testPreferences({ agentNotifications: false }) });
        transport.emit("agent_notice", notice);
        expect(appWindow.requestUserAttention).not.toHaveBeenCalled();
        expect(notification.sent).not.toHaveBeenCalled();
    });

    it("badges the Dock with the agents the core shows waiting or done, and clears it once none are", () => {
        setState({ agentStatus: shown("a2", "blocked") });
        expect(appWindow.setBadgeCount).toHaveBeenLastCalledWith(1);
        setState({ agentStatus: shown("a2", "working") });
        expect(appWindow.setBadgeCount).toHaveBeenLastCalledWith(undefined);
    });
});

describe("the first launch", () => {
    it("greets once with a bip, so macOS asks for permission up front", async () => {
        introduceNotifications();
        introduceNotifications();
        await vi.waitFor(() => expect(notification.sent).toHaveBeenCalledTimes(1));
        expect(notification.sent.mock.calls[0][0].title).toBe("Sikemux notifications are on");
        expect(notification.bips).toHaveBeenCalledTimes(1);
        expect(getState().preferences?.notificationsIntroduced).toBe(true);
    });

    it("stays quiet for someone who switched notifications off", () => {
        setState({ preferences: testPreferences({ agentNotifications: false }) });
        introduceNotifications();
        expect(notification.sent).not.toHaveBeenCalled();
        expect(notification.bips).not.toHaveBeenCalled();
        expect(getState().preferences?.notificationsIntroduced).toBe(false);
    });
});
