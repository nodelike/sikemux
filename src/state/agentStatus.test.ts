import { describe, expect, it } from "vitest";
import { acknowledgeAgentState, agentDisplay, reduceAgentState, rollupAgentDisplays, type AgentStateEvent } from "./agentStatus";

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
        expect(rollupAgentDisplays([agentDisplay(idle.state), agentDisplay(blocked.state)])?.group).toBe("blocked");
    });

    it("keeps process exit distinct from ready and completion", () => {
        const ready = reduceAgentState(undefined, event("idle", 1), true)!;
        const stopped = reduceAgentState(ready, event("stopped", 2), false)!;
        expect(stopped).toMatchObject({ state: "stopped", backendState: "stopped", unread: false });
        expect(agentDisplay(stopped.state)).toMatchObject({ group: "idle", label: "Idle" });
    });

    it("keeps an agent with only leftover shells or subagents in its own group", () => {
        expect(agentDisplay("idle", { tasks: 2, subagents: 1 })).toMatchObject({ group: "idle", tasks: 2, subagents: 1 });
        expect(agentDisplay("done", { tasks: 1 }).group).toBe("done");
        expect(agentDisplay(undefined).group).toBe("idle");
    });
});
