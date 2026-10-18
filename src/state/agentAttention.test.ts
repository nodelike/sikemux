import { beforeEach, describe, expect, it } from "vitest";
import { activeAgentId } from "./selectors";
import { revealAgent } from "./commands";
import { getState, setState } from "./store";
import { withAgents } from "../test/agents";
import type { AgentRuntimeState } from "./types";

const initial = getState();

function blocked(): AgentRuntimeState {
    return { state: "blocked", backendState: "blocked", unread: true, updatedAt: 0, sequence: 1, source: "screen", confidence: "high", reason: "" };
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

describe("an agent waiting in another project", () => {
    it("switches project and opens the agent that was waiting", () => {
        setState({ agentActivity: { a2: blocked() } });

        revealAgent("a2");

        expect(getState().activeSessionId).toBe("s2");
        expect(activeAgentId(getState(), getState().sessions.s2)).toBe("a2");
    });
});
