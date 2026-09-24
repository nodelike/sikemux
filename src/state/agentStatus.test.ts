import { describe, expect, it } from "vitest";
import { acknowledgeAgentState, reduceAgentState, rollupAgentStates, sortByAttention, type AgentStateEvent } from "./agentStatus";

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

    it("keeps process exit distinct from ready and completion", () => {
        const ready = reduceAgentState(undefined, event("idle", 1), true)!;
        const stopped = reduceAgentState(ready, event("stopped", 2), false)!;
        expect(stopped).toMatchObject({ state: "stopped", backendState: "stopped", unread: false });
        expect(rollupAgentStates([ready, stopped])).toBe("stopped");
    });

    it("orders open agents by what needs attention, then by when they last ran", () => {
        const at = (state: AgentStateEvent["state"], workedAt: number, settledAt: number) => {
            const worked = reduceAgentState(undefined, event("working", 1), true, workedAt)!;
            return state === "working" ? worked : reduceAgentState(worked, event(state, 2), false, settledAt)!;
        };
        const activity = {
            stale: at("idle", 10, 11),
            fresh: at("idle", 50, 51),
            finished: at("idle", 5, 6),
            asking: at("blocked", 1, 2),
            running: at("working", 3, 3),
        };
        const acknowledged = {
            ...activity,
            finished: acknowledgeAgentState(activity.finished),
            fresh: acknowledgeAgentState(activity.fresh),
            stale: acknowledgeAgentState(activity.stale),
        };
        const ids = ["never", "stale", "finished", "fresh", "running", "asking", "shell"].map((id) => ({ id }));
        expect(sortByAttention(ids, activity, { shell: 1 }).map((a) => a.id)).toEqual([
            "asking",
            "running",
            "shell",
            "fresh",
            "stale",
            "finished",
            "never",
        ]);
        expect(sortByAttention(ids, acknowledged, {}).map((a) => a.id)).toEqual([
            "asking",
            "running",
            "fresh",
            "stale",
            "finished",
            "never",
            "shell",
        ]);
    });
});
