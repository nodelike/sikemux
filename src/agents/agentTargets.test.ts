import { describe, expect, it } from "vitest";
import type { AgentInfo } from "../api/agents";
import type { Agent, Session, Window } from "../state/types";
import { agentChoices, codeFence, nearestProjectSessionId, projectSessionForCwd, sessionOfPane } from "./agentTargets";

function agentWindow(id: string, agentId: string): Window {
    return { id, name: id, role: "agent", activePaneId: agentId, root: { type: "pane", id: agentId, cwd: "/repo", kind: "agent", title: agentId } };
}

function agent(id: string, title: string, type: Agent["type"] = "claude"): Agent {
    return { id, type, title, startup: "" };
}

function info(type: AgentInfo["type"], label: string, available = true): AgentInfo {
    return { type, label, command: type, available, defaultModel: null, defaultEffort: null };
}

const project: Session = { id: "s1", name: "repo", kind: "project", cwd: "/repo", pinned: false, activeWindowId: "w2" };

const state = {
    sessions: { s1: project, s2: { ...project, id: "s2", kind: "command" as const } },
    windows: { w1: agentWindow("w1", "a1"), w2: agentWindow("w2", "a2") },
    windowsBySession: { s1: ["w1", "w2"] },
    agents: { a1: agent("a1", "Fix the parser"), a2: agent("a2", "Write docs", "codex") },
};

describe("agentChoices", () => {
    it("lists the open agents with the one in front first, then a new chat per installed CLI", () => {
        const choices = agentChoices(state, "s1", [info("claude", "Claude"), info("codex", "Codex", false), info("pi", "Pi")]);
        expect(choices.map((c) => c.label)).toEqual(["Write docs", "Fix the parser", "New Claude chat", "New Pi chat"]);
        expect(choices[0].target).toEqual({ agentId: "a2" });
        expect(choices[2].target).toEqual({ newAgent: "claude", sessionId: "s1" });
    });

    it("offers nothing outside a project", () => {
        expect(agentChoices(state, "s2", [info("claude", "Claude")])).toEqual([]);
        expect(agentChoices(state, "missing", [info("claude", "Claude")])).toEqual([]);
    });
});

describe("nearestProjectSessionId", () => {
    const sessions = { p1: project, p2: { ...project, id: "p2" }, x: { ...project, id: "x", kind: "command" as const } };

    it("prefers the project in front, then the last one in front, then the first one open", () => {
        expect(nearestProjectSessionId({ sessions, sessionOrder: ["x", "p1", "p2"], activeSessionId: "p2", lastSessionId: "p1" })).toBe("p2");
        expect(nearestProjectSessionId({ sessions, sessionOrder: ["x", "p1", "p2"], activeSessionId: "x", lastSessionId: "p2" })).toBe("p2");
        expect(nearestProjectSessionId({ sessions, sessionOrder: ["x", "p1", "p2"], activeSessionId: "x", lastSessionId: null })).toBe("p1");
        expect(nearestProjectSessionId({ sessions: { x: sessions.x }, sessionOrder: ["x"], activeSessionId: "x", lastSessionId: null })).toBeNull();
    });
});

describe("codeFence", () => {
    it("fences text and trims trailing newlines", () => {
        expect(codeFence("a\nb\n\n", "diff")).toBe("```diff\na\nb\n```");
    });

    it("outgrows any backtick run inside the text", () => {
        expect(codeFence("x ```` y")).toBe("`````\nx ```` y\n`````");
    });
});

describe("sessionOfPane", () => {
    it("finds the session holding the pane's window", () => {
        expect(sessionOfPane({ ...state, sessionOrder: ["s1"] }, "a1")).toBe("s1");
        expect(sessionOfPane({ ...state, sessionOrder: ["s1"] }, "nope")).toBeNull();
    });
});

describe("projectSessionForCwd", () => {
    it("picks the project open in that folder over the one in front", () => {
        const sessions = { p1: project, p2: { ...project, id: "p2", cwd: "/other" } };
        const base = { sessions, sessionOrder: ["p1", "p2"], activeSessionId: "p1", lastSessionId: null };
        expect(projectSessionForCwd(base, "/other")).toBe("p2");
        expect(projectSessionForCwd(base, "/nowhere")).toBe("p1");
        expect(projectSessionForCwd(base, null)).toBe("p1");
    });
});
