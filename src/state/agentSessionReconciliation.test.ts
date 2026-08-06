import { beforeEach, describe, expect, it } from "vitest";
import { agentSessionMetadataPending, reconcileAgentSessions } from "./commands";
import { getState, setState } from "./store";

const initial = getState();

beforeEach(() => setState(initial, true));

describe("agent session reconciliation", () => {
    it("replaces a generic new-agent title when the first prompt reaches disk", () => {
        const state = getState();
        const sessionId = state.activeSessionId;
        const agentId = "agent-new";
        const transcriptId = "019fd81d-c861-7920-bc88-a41a6b17aca4";
        setState({
            sessions: {
                ...state.sessions,
                [sessionId]: { ...state.sessions[sessionId], kind: "project", cwd: "/repo", activeAgentId: agentId, view: "agent" },
            },
            agents: {
                [agentId]: {
                    id: agentId,
                    type: "codex",
                    title: "codex",
                    startup: "codex",
                    createdAt: 100_000,
                    baselineSessionIds: [],
                    launchState: "live",
                },
            },
            agentsBySession: { ...state.agentsBySession, [sessionId]: [agentId] },
        });

        reconcileAgentSessions("codex", "/repo", [{ id: transcriptId, title: transcriptId.slice(0, 8), mtime: 101 }]);
        expect(getState().agents[agentId]).toMatchObject({ resumeId: transcriptId, title: "codex" });
        expect(agentSessionMetadataPending(getState().agents[agentId])).toBe(true);

        reconcileAgentSessions("codex", "/repo", [{ id: transcriptId, title: "Hello", mtime: 102 }]);
        expect(getState().agents[agentId]).toMatchObject({ resumeId: transcriptId, title: "Hello" });
        expect(agentSessionMetadataPending(getState().agents[agentId])).toBe(false);
    });
});
