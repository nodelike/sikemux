import { beforeEach, describe, expect, it } from "vitest";
import { clearAgentUnread, noteAgentActivity, selectAgent } from "./commands";
import { getState, setState } from "./store";

const initial = getState();

beforeEach(() => setState(initial, true));

function installAgent() {
    const state = getState();
    const sid = state.activeSessionId;
    const agent = { id: "agent-1", type: "claude" as const, title: "Claude", startup: "claude" };
    setState({
        sessions: { ...state.sessions, [sid]: { ...state.sessions[sid], kind: "project", view: "windows", activeAgentId: agent.id } },
        agents: { [agent.id]: agent },
        agentsBySession: { ...state.agentsBySession, [sid]: [agent.id] },
    });
    return agent.id;
}

describe("agent activity", () => {
    it("shows working immediately and marks a hidden completion unread", () => {
        const id = installAgent();
        noteAgentActivity(id, "working");
        expect(getState().agentActivity[id]).toMatchObject({ state: "working", unread: false });

        noteAgentActivity(id, "complete");
        expect(getState().agentActivity[id]).toMatchObject({ state: "done", backendState: "idle", unread: true });
    });

    it("clears unread on selection and keeps visible completions read", () => {
        const id = installAgent();
        noteAgentActivity(id, "complete");
        selectAgent(id);
        expect(getState().agentActivity[id].unread).toBe(false);

        noteAgentActivity(id, "working");
        noteAgentActivity(id, "complete");
        expect(getState().agentActivity[id]).toMatchObject({ state: "idle", unread: false });

        clearAgentUnread(id);
        expect(getState().agentActivity[id].unread).toBe(false);
    });
});
