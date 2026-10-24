import { describe, expect, it } from "vitest";
import type { AgentInfo } from "../api/agents";
import { withMissingAgents } from "./setupList";

const agent = (type: AgentInfo["type"], status: AgentInfo["status"], available = true): AgentInfo => ({
    type,
    label: type,
    command: type,
    available,
    defaultModel: null,
    defaultEffort: null,
    status,
});

describe("the agents Sikemux can set up", () => {
    it("adds every agent it can install but did not find", () => {
        const all = withMissingAgents([agent("claude", { state: "ready", account: null })]);
        expect(all.map((each) => each.type)).toEqual(["claude", "codex", "hermes", "pi", "opencode", "omp", "grok"]);
        expect(all[1]).toMatchObject({ available: false, status: { state: "missing" } });
    });
});
