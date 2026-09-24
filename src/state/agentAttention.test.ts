import { beforeEach, describe, expect, it } from "vitest";
import { activeAgentId, agentsAwaitingInput } from "./selectors";
import { revealAgent } from "./commands";
import { getState, setState } from "./store";
import { withAgents } from "../test/agents";
import type { AgentRuntimeState } from "./types";

const initial = getState();

function blocked(): AgentRuntimeState {
    return { state: "blocked", backendState: "blocked", unread: true, updatedAt: 0, sequence: 1, source: "screen", confidence: "high", reason: "" };
}

function working(): AgentRuntimeState {
    return { ...blocked(), state: "working", backendState: "working" };
}

/** Two projects, one agent each, so a blocked agent can be off-screen. */
function twoProjects(): void {
    setState(initial, true);
    const first = getState().activeSessionId;
    setState((state) => ({
        sessionOrder: [first, "s2"],
        sessions: { ...state.sessions, s2: { ...state.sessions[first], id: "s2", name: "openjob" } },
    }));
    setState((state) => withAgents(state, first, [{ id: "a1", type: "claude", title: "claude · api", startup: "claude", cwd: "/one" }]));
    setState((state) => withAgents(state, "s2", [{ id: "a2", type: "codex", title: "codex · web", startup: "codex", cwd: "/two" }]));
}

beforeEach(twoProjects);

describe("agents awaiting input", () => {
    it("is empty while nothing is blocked", () => {
        expect(agentsAwaitingInput(getState())).toEqual([]);
    });

    it("reports a blocked agent with the project it belongs to", () => {
        setState({ agentActivity: { a1: blocked() } });

        const found = agentsAwaitingInput(getState());

        expect(found).toHaveLength(1);
        expect(found[0]).toMatchObject({ agentId: "a1", agentTitle: "claude · api", agentType: "claude" });
    });

    it("gathers blocked agents from every project, not just the active one", () => {
        setState({ agentActivity: { a1: blocked(), a2: blocked() } });

        expect(agentsAwaitingInput(getState()).map((entry) => entry.agentId)).toEqual(["a1", "a2"]);
    });

    it("ignores an agent that is merely working", () => {
        setState({ agentActivity: { a1: working() } });

        expect(agentsAwaitingInput(getState())).toEqual([]);
    });

    it("switches project and opens the agent that was waiting", () => {
        const first = getState().activeSessionId;
        setState({ agentActivity: { a2: blocked() } });

        revealAgent("a2");

        expect(getState().activeSessionId).toBe("s2");
        expect(getState().activeSessionId).not.toBe(first);
        expect(activeAgentId(getState(), getState().sessions.s2)).toBe("a2");
    });
});
