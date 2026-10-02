import { beforeEach, describe, expect, it } from "vitest";
import * as cmd from "../commands";
import { agentIdsOf } from "../selectors";
import { getState, setState } from "../store";
import type { Agent, Session } from "../types";

const initial = getState();

beforeEach(() => setState(initial, true));

const project: Session = { id: "p", name: "sikemux", kind: "project", cwd: "/Users/edon/code/sikemux", pinned: false, activeWindowId: "" };

function agent(resumeId?: string): Agent {
    return { id: "agent-1", type: "claude", title: "Fix the rail", startup: "", resumeId, createdAt: 0, launchState: "live" } as Agent;
}

describe("agent deep links", () => {
    it("has no link until the agent has a conversation to come back to", () => {
        expect(cmd.agentLink(agent(), project)).toBeNull();
        expect(cmd.agentLink(agent("abc"), { ...project, kind: "ssh" })).toBeNull();
    });

    it("carries the agent type, its conversation and its project", () => {
        expect(cmd.agentLink(agent("a b/c"), project)).toBe("sikemux://agent/claude/a%20b%2Fc?project=%2FUsers%2Fedon%2Fcode%2Fsikemux");
    });

    it("opens the project and resumes the conversation the link names", () => {
        const link = cmd.agentLink(agent("session-42"), project)!;
        expect(cmd.routeDeepLink(link)).toBe(true);
        const state = getState();
        const session = state.sessions[state.activeSessionId];
        expect(session?.cwd).toBe(project.cwd);
        const opened = agentIdsOf(state, session!.id).map((id) => state.agents[id]);
        expect(opened).toEqual([expect.objectContaining({ type: "claude", resumeId: "session-42" })]);
    });

    it("focuses the agent already open on that conversation instead of opening another", () => {
        const link = cmd.agentLink(agent("session-42"), project)!;
        cmd.routeDeepLink(link);
        cmd.routeDeepLink(link);
        const state = getState();
        expect(agentIdsOf(state, state.activeSessionId)).toHaveLength(1);
    });

    it("turns down links it does not understand", () => {
        expect(cmd.routeDeepLink("sikemux://agent/nobody/1?project=%2Ftmp")).toBe(false);
        expect(cmd.routeDeepLink("sikemux://agent/claude?project=%2Ftmp")).toBe(false);
        expect(cmd.routeDeepLink("sikemux://agent/claude/1")).toBe(false);
        expect(cmd.routeDeepLink("https://example.com/")).toBe(false);
        expect(cmd.routeDeepLink("not a link")).toBe(false);
    });
});
