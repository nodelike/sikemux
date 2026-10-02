import { createElement } from "react";
import { act, cleanup, render } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { agentApi } from "../api/agents";
import { getState, setState } from "../state/store";
import { agentWindowId } from "../state/selectors";
import { withAgents } from "../test/agents";
import {
    AGENT_IDLE_SLEEP_MS,
    MAX_WARM_IDLE_AGENTS,
    AgentLifecycleManager,
    agentIdsToAutoSleep,
    reconcileHiddenAgentTimes,
    type HiddenAgentTimes,
} from "./AgentLifecycleManager";
import { agentIdsWithLiveSessions } from "../state/agentLiveSessions";

vi.mock("../api/agents", () => ({ agentApi: { liveSessions: vi.fn(async () => []) } }));

const liveSessions = vi.mocked(agentApi.liveSessions);

const initial = getState();

beforeEach(() => {
    vi.useRealTimers();
    setState(initial, true);
    liveSessions.mockReset();
    liveSessions.mockResolvedValue([]);
});

afterEach(() => {
    cleanup();
    vi.useRealTimers();
});

function arrangeAgents(count: number) {
    const state = getState();
    const sessionId = state.activeSessionId;
    const agents = Object.fromEntries(
        Array.from({ length: count }, (_, index) => {
            const id = `agent-${index}`;
            return [
                id,
                {
                    id,
                    type: "codex" as const,
                    title: id,
                    startup: "codex",
                    resumeId: `session-${index}`,
                    launchState: "live" as const,
                },
            ];
        }),
    );
    const agentActivity = Object.fromEntries(
        Object.keys(agents).map((id, index) => [
            id,
            {
                state: "idle" as const,
                backendState: "idle" as const,
                unread: false,
                updatedAt: index,
                sequence: 1,
                source: "screen" as const,
                confidence: "high" as const,
                reason: "prompt visible",
            },
        ]),
    );
    setState({
        sessions: { ...state.sessions, [sessionId]: { ...state.sessions[sessionId], kind: "project" } },
        ...withAgents(state, sessionId, Object.values(agents)),
        agentActivity,
    });
}

function asClaude(id: string) {
    setState((state) => ({ agents: { ...state.agents, [id]: { ...state.agents[id], type: "claude" as const } } }));
}

function focusAgent(id: string) {
    setState((state) => ({
        sessions: {
            ...state.sessions,
            [state.activeSessionId]: { ...state.sessions[state.activeSessionId], activeWindowId: agentWindowId(state, id)! },
        },
    }));
}

function sleep(id: string) {
    setState((state) => ({ agents: { ...state.agents, [id]: { ...state.agents[id], launchState: "dormant" } } }));
}

describe("agent sleep policy", () => {
    it("enforces the timeout while mounted", async () => {
        vi.useFakeTimers();
        vi.setSystemTime(0);
        arrangeAgents(1);
        render(createElement(AgentLifecycleManager));

        await act(async () => vi.advanceTimersByTimeAsync(AGENT_IDLE_SLEEP_MS));

        expect(getState().agents["agent-0"].launchState).toBe("dormant");
    });

    it("sleeps a hidden resumable agent after the idle timeout", () => {
        arrangeAgents(1);
        const now = 1_000_000;
        const hiddenSince: HiddenAgentTimes = new Map([["agent-0", now - AGENT_IDLE_SLEEP_MS]]);
        expect(agentIdsToAutoSleep(getState(), hiddenSince, now)).toEqual(["agent-0"]);
    });

    it("bounds hidden warm idle agents by least-recent use", () => {
        arrangeAgents(MAX_WARM_IDLE_AGENTS + 2);
        const hiddenSince: HiddenAgentTimes = new Map(Object.keys(getState().agents).map((id, index) => [id, 100 + index] as const));
        expect(agentIdsToAutoSleep(getState(), hiddenSince, 1_000)).toEqual(["agent-0", "agent-1"]);
    });

    it("preserves visible, unsafe, unresumable, and kept-awake agents", () => {
        arrangeAgents(5);
        const state = getState();
        const sessionId = state.activeSessionId;
        setState((current) => ({
            sessions: {
                ...current.sessions,
                [sessionId]: { ...current.sessions[sessionId], activeWindowId: agentWindowId(current, "agent-0")! },
            },
            agents: {
                ...current.agents,
                "agent-1": { ...current.agents["agent-1"], keepAlive: true },
                "agent-2": { ...current.agents["agent-2"], resumeId: undefined },
            },
            agentActivity: {
                ...current.agentActivity,
                "agent-3": { ...current.agentActivity["agent-3"], backendState: "working", state: "working" },
                "agent-4": { ...current.agentActivity["agent-4"], confidence: "low" },
            },
        }));
        const hiddenSince: HiddenAgentTimes = new Map(Object.keys(getState().agents).map((id) => [id, 0]));
        reconcileHiddenAgentTimes(getState(), hiddenSince, AGENT_IDLE_SLEEP_MS);
        expect(agentIdsToAutoSleep(getState(), hiddenSince, AGENT_IDLE_SLEEP_MS * 2)).toEqual([]);
        expect(hiddenSince.has("agent-0")).toBe(false);
    });

    it("resumes the agent the user switches to", () => {
        arrangeAgents(2);
        focusAgent("agent-0");
        sleep("agent-1");
        render(createElement(AgentLifecycleManager));

        act(() => focusAgent("agent-1"));

        expect(getState().agents["agent-1"].launchState).toBe("live");
    });

    it("leaves the focused agent asleep once it is put to sleep", () => {
        arrangeAgents(2);
        focusAgent("agent-0");
        render(createElement(AgentLifecycleManager));

        act(() => sleep("agent-0"));

        expect(getState().agents["agent-0"].launchState).toBe("dormant");
    });

    it("keeps an agent that still has a shell, monitor or subagent running", () => {
        arrangeAgents(1);
        setState({ agentBackgroundWork: { "agent-0": 1 } });
        const hiddenSince: HiddenAgentTimes = new Map([["agent-0", 0]]);
        expect(agentIdsToAutoSleep(getState(), hiddenSince, AGENT_IDLE_SLEEP_MS * 2)).toEqual([]);
    });

    it("drops stale and sleeping entries from hidden-time tracking", () => {
        arrangeAgents(2);
        setState((state) => ({
            agents: { ...state.agents, "agent-1": { ...state.agents["agent-1"], launchState: "dormant" } },
        }));
        const hiddenSince: HiddenAgentTimes = new Map([
            ["agent-0", 10],
            ["agent-1", 10],
            ["removed", 10],
        ]);
        reconcileHiddenAgentTimes(getState(), hiddenSince, 20);
        expect([...hiddenSince]).toEqual([["agent-0", 10]]);
    });
});

describe("what claude says about its own sessions", () => {
    it("holds back an agent whose session is still running a background shell", async () => {
        arrangeAgents(1);
        asClaude("agent-0");
        liveSessions.mockResolvedValue([{ sessionId: "session-0", status: "shell" }]);

        expect([...(await agentIdsWithLiveSessions(getState(), ["agent-0"]))]).toEqual(["agent-0"]);
    });

    it("lets an agent whose session reads idle go", async () => {
        arrangeAgents(1);
        asClaude("agent-0");
        liveSessions.mockResolvedValue([{ sessionId: "session-0", status: "idle" }]);

        expect([...(await agentIdsWithLiveSessions(getState(), ["agent-0"]))]).toEqual([]);
    });

    it("asks nothing of an agent that is not claude", async () => {
        arrangeAgents(1);

        expect([...(await agentIdsWithLiveSessions(getState(), ["agent-0"]))]).toEqual([]);
        expect(liveSessions).not.toHaveBeenCalled();
    });

    it("leaves the screen's reading alone when claude cannot answer", async () => {
        arrangeAgents(1);
        asClaude("agent-0");
        liveSessions.mockRejectedValue(new Error("claude is not available"));

        expect([...(await agentIdsWithLiveSessions(getState(), ["agent-0"]))]).toEqual([]);
    });

    it("sleeps a hidden agent whose session has nothing left running", async () => {
        vi.useFakeTimers();
        vi.setSystemTime(0);
        arrangeAgents(1);
        asClaude("agent-0");
        liveSessions.mockResolvedValue([{ sessionId: "session-0", status: "idle" }]);
        render(createElement(AgentLifecycleManager));

        await act(async () => vi.advanceTimersByTimeAsync(AGENT_IDLE_SLEEP_MS));

        expect(getState().agents["agent-0"].launchState).toBe("dormant");
    });

    it("does not sleep a hidden agent that is still running a shell", async () => {
        vi.useFakeTimers();
        vi.setSystemTime(0);
        arrangeAgents(1);
        asClaude("agent-0");
        liveSessions.mockResolvedValue([{ sessionId: "session-0", status: "shell" }]);
        render(createElement(AgentLifecycleManager));

        await act(async () => vi.advanceTimersByTimeAsync(AGENT_IDLE_SLEEP_MS));

        expect(getState().agents["agent-0"].launchState).toBe("live");
    });
});
