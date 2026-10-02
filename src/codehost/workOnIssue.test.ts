import { describe, expect, it } from "vitest";
import type { AgentInfo } from "../api/agents";
import { chatAgentFor, projectSessionIn } from "./workOnIssue";

const info = (type: AgentInfo["type"], available = true): AgentInfo =>
    ({ type, label: type, command: type, available, defaultModel: null }) as AgentInfo;

describe("chatAgentFor", () => {
    it("takes the agent launched last when it can chat", () => {
        expect(chatAgentFor([info("claude"), info("codex")], "codex")).toBe("codex");
    });

    it("falls back to the first installed agent that can chat", () => {
        expect(chatAgentFor([info("pi"), info("claude", false), info("codex")], "pi")).toBe("codex");
        expect(chatAgentFor([info("claude")], null)).toBe("claude");
    });

    it("finds none when no chat agent is installed", () => {
        expect(chatAgentFor([info("pi")], null)).toBeNull();
    });
});

describe("projectSessionIn", () => {
    it("finds the project open in the folder and nothing else", () => {
        const state = {
            sessionOrder: ["s1", "s2"],
            sessions: {
                s1: { id: "s1", kind: "command", cwd: "/repo" },
                s2: { id: "s2", kind: "project", cwd: "/repo" },
            },
        } as unknown as Parameters<typeof projectSessionIn>[0];
        expect(projectSessionIn(state, "/repo")).toBe("s2");
        expect(projectSessionIn(state, "/other")).toBeNull();
    });
});
