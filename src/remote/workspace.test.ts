import { describe, expect, it } from "vitest";
import type { Agent, ProviderProfile, Session } from "../state/types";
import { remoteChats, remoteRecent, remoteWorkspace } from "./workspace";

function project(id: string, cwd: string): Session {
    return { id, name: cwd.split("/").at(-1) ?? id, kind: "project", cwd, pinned: false, activeWindowId: `${id}-window` };
}

const sessions: Record<string, Session> = {
    a: project("a", "/Users/me/sikemux"),
    ssh: { ...project("ssh", ""), kind: "ssh", name: "gpu-box" },
    b: project("b", "/Users/me/site"),
};

function profile(id: string, name: string, provider: ProviderProfile["provider"]): ProviderProfile {
    return { id, name, provider, accent: "#fff", configPath: `~/.${id}`, environmentKeys: ["ANTHROPIC_API_KEY"] };
}

describe("remoteWorkspace", () => {
    it("offers the open local projects in tab order and never an SSH host", () => {
        const { projects } = remoteWorkspace(sessions, ["b", "ssh", "a"], [], "bypass");
        expect(projects).toEqual([
            { id: "b", name: "site", path: "/Users/me/site" },
            { id: "a", name: "sikemux", path: "/Users/me/sikemux" },
        ]);
    });

    it("offers each chat agent once, or once per profile, with the default permission mode it supports", () => {
        const profiles = [profile("work", "Work", "claude"), profile("home", "Home", "claude"), profile("cx", "Default", "codex")];
        const { launchers } = remoteWorkspace(sessions, [], profiles, "bypass");
        const byId = Object.fromEntries(launchers.map((launcher) => [launcher.id, launcher]));
        expect(Object.keys(byId).sort()).toEqual(["claude:home", "claude:work", "codex:cx", "grok", "hermes", "omp", "opencode"]);
        expect(byId["claude:work"]).toMatchObject({
            provider: "claude",
            label: "Claude · Work",
            configPath: "~/.work",
            environmentKeys: ["ANTHROPIC_API_KEY"],
        });
        expect(byId["codex:cx"].label).toBe("Codex");
        expect(byId["opencode"].permissionMode).toBe("workspace-write");
        expect(byId["grok"].permissionMode).toBe("bypass");
    });

    it("tells devices whether each launcher's agent is signed in, by profile", () => {
        const profiles = [profile("work", "Work", "claude"), profile("home", "Home", "claude")];
        const catalog = [
            { type: "claude", label: "Claude", command: "claude", configPath: "~/.work", status: { state: "signedOut" } },
            { type: "claude", label: "Claude", command: "claude", configPath: "~/.home", status: { state: "ready", account: null } },
            { type: "opencode", label: "OpenCode", command: "opencode", status: { state: "unknown" } },
        ] as unknown as Parameters<typeof remoteWorkspace>[4];
        const { launchers } = remoteWorkspace(sessions, [], profiles, "bypass", catalog);
        const byId = Object.fromEntries(launchers.map((launcher) => [launcher.id, launcher]));
        expect(byId["claude:work"].status).toBe("signedOut");
        expect(byId["claude:home"].status).toBe("ready");
        expect(byId["opencode"].status).toBe("unknown");
        expect(byId["hermes"].status).toBeUndefined();
    });
});

describe("remoteChats", () => {
    it("lists chat agents by the rail's name, sleeping ones included, and leaves terminal-only agents out", () => {
        const agent = (id: string, type: string, title: string, launchState?: "dormant") => ({ id, type, title, startup: "", launchState }) as Agent;
        const agentWindow = (id: string) => ({ id, role: "agent", root: { type: "pane", id, kind: "agent", title: "" } });
        const chats = remoteChats({
            agents: {
                a: agent("a", "claude", "Fix the login flow"),
                b: agent("b", "claude", "claude", "dormant"),
                c: agent("c", "shell", "zsh"),
                d: agent("d", "claude", "Not in any window"),
            },
            windows: { a: agentWindow("a"), b: agentWindow("b"), c: agentWindow("c") } as never,
            sessions,
            sessionOrder: ["a"],
            windowsBySession: { a: ["a", "b", "c"] },
        });
        expect(chats).toEqual([
            { agentId: "a", provider: "claude", title: "Fix the login flow", cwd: "/Users/me/sikemux", asleep: false },
            { agentId: "b", provider: "claude", title: null, cwd: "/Users/me/sikemux", asleep: true },
        ]);
    });
});

describe("remoteRecent", () => {
    it("resumes each saved chat with the launcher of the profile the rail lists it under", () => {
        const profiles = [profile("work", "Work", "claude"), profile("home", "Home", "claude")];
        const chat = (agent: "claude" | "opencode", id: string, title: string) => ({
            agent,
            id,
            title,
            mtime: 1_700_000_000,
            project: "/Users/me/sikemux",
        });
        const recent = remoteRecent([chat("claude", "c1", "Fix the login flow"), chat("opencode", "o1", "x".repeat(300))], profiles, {
            claude: "home",
        });
        expect(recent[0]).toEqual({
            launcher: "claude:home",
            provider: "claude",
            sessionId: "c1",
            title: "Fix the login flow",
            cwd: "/Users/me/sikemux",
            activeAt: 1_700_000_000_000,
        });
        expect(recent[1].launcher).toBe("opencode");
        expect(recent[1].title).toHaveLength(200);
        expect(remoteRecent([chat("claude", "c1", "t")], profiles, {})[0].launcher).toBe("claude:work");
    });
});
