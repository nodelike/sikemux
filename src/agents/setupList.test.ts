import { describe, expect, it } from "vitest";
import type { AgentInfo } from "../api/agents";
import { setupOrder, withMissingAgents } from "./setupList";

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

    it("puts ready agents first, then Claude and Codex, then the rest", () => {
        const ordered = setupOrder([
            agent("grok", { state: "missing" }, false),
            agent("codex", { state: "signedOut" }),
            agent("opencode", { state: "ready", account: "apiKey" }),
            agent("hermes", { state: "missing" }, false),
            agent("claude", { state: "missing" }, false),
        ]);
        expect(ordered.map((each) => each.type)).toEqual(["opencode", "claude", "codex", "hermes", "grok"]);
    });
});
