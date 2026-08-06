import { describe, expect, it } from "vitest";
import { acknowledgeAgentState, reduceAgentState, rollupAgentStates, type AgentStateEvent } from "./agentStatus";

const event = (state: AgentStateEvent["state"], sequence: number): AgentStateEvent => ({
    agentId: "a",
    state,
    sequence,
    source: "screen",
    confidence: "high",
    reason: "fixture",
});

describe("semantic agent presentation", () => {
    it("turns hidden working to idle into unseen done", () => {
        const working = reduceAgentState(undefined, event("working", 1), false, 1)!;
        const done = reduceAgentState(working, event("idle", 2), false, 2)!;
        expect(done).toMatchObject({ state: "done", backendState: "idle", unread: true });
        expect(acknowledgeAgentState(done)).toMatchObject({ state: "idle", unread: false });
    });

    it("keeps visible completion idle and rejects stale events", () => {
        const working = reduceAgentState(undefined, event("working", 2), true)!;
        expect(reduceAgentState(working, event("idle", 3), true)?.state).toBe("idle");
        expect(reduceAgentState(working, event("blocked", 2), false)).toBeUndefined();
    });

    it("rolls up by attention priority", () => {
        const idle = reduceAgentState(undefined, event("idle", 1), true)!;
        const blocked = reduceAgentState(undefined, event("blocked", 2), false)!;
        expect(rollupAgentStates([idle, blocked])).toBe("blocked");
    });
});
