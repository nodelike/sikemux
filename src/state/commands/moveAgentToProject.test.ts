import { beforeEach, describe, expect, it } from "vitest";
import * as cmd from "../commands";
import { getState, setState } from "../store";
import { agentWindowId, ownerSessionId } from "../selectors";

const initial = getState();

beforeEach(() => setState(initial, true));

function newChatIn(cwd: string): string {
    cmd.createProjectSession(cwd);
    cmd.addAgent("claude");
    const state = getState();
    const winId = state.sessions[state.activeSessionId].activeWindowId;
    return Object.values(state.agents).find((agent) => agentWindowId(state, agent.id) === winId)!.id;
}

function projectOf(agentId: string): string | undefined {
    const state = getState();
    const sessionId = ownerSessionId(state, agentWindowId(state, agentId)!);
    return sessionId ? state.sessions[sessionId].cwd : undefined;
}

describe("moveAgentToProject", () => {
    it("opens the project and carries the new chat into it", () => {
        const agentId = newChatIn("/work/app");
        const from = getState().activeSessionId;

        cmd.moveAgentToProject(agentId, "/work/site");

        const state = getState();
        const to = state.sessions[state.activeSessionId];
        expect(to.cwd).toBe("/work/site");
        expect(projectOf(agentId)).toBe("/work/site");
        expect(to.activeWindowId).toBe(agentWindowId(state, agentId));
        expect(state.agents[agentId].cwd).toBe("/work/site");
        expect(state.windows[agentWindowId(state, agentId)!].root).toMatchObject({ cwd: "/work/site" });
        expect(state.windowsBySession[from].map((id) => state.windows[id].role)).toEqual(["term"]);
        expect(state.sessions[from].activeWindowId).toBe(state.windowsBySession[from][0]);
    });

    it("joins a project that is already open instead of opening it twice", () => {
        cmd.createProjectSession("/work/site");
        const agentId = newChatIn("/work/app");

        cmd.moveAgentToProject(agentId, "/work/site");

        const state = getState();
        expect(Object.values(state.sessions).filter((session) => session.cwd === "/work/site")).toHaveLength(1);
        expect(projectOf(agentId)).toBe("/work/site");
    });

    it("leaves a chat that has already started where it is", () => {
        const agentId = newChatIn("/work/app");
        setState({ agents: { ...getState().agents, [agentId]: { ...getState().agents[agentId], resumeId: "abc" } } });

        cmd.moveAgentToProject(agentId, "/work/site");

        expect(projectOf(agentId)).toBe("/work/app");
        expect(getState().agents[agentId].cwd).toBe("/work/app");
    });
});
