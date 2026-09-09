import { beforeEach, describe, expect, it, vi } from "vitest";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import { applyHydrate, flushPersist, hydrationAllowsPersistence, resetPersistenceForTests, subscribePersist } from "./persist";
import * as cmd from "./commands";
import { getState, setState } from "./store";
import { useToasts } from "./toast";

function deferred<T>() {
    let resolve!: (value: T) => void;
    let reject!: (reason?: unknown) => void;
    const promise = new Promise<T>((res, rej) => {
        resolve = res;
        reject = rej;
    });
    return { promise, resolve, reject };
}

const initial = getState();

beforeEach(() => {
    vi.useRealTimers();
    invoke.mockReset();
    resetPersistenceForTests();
    setState(initial, true);
    useToasts.setState({ toasts: [] });
});

describe("frontend persistence", () => {
    it("omits transient task terminals and restores a durable active window", async () => {
        cmd.createProjectSession("/work/demo");
        const paneId = cmd.openTaskTerminal({
            executionId: "execution-secret-free",
            terminalKey: "task:test:/work/demo",
            taskId: "test",
            label: "Test",
            project: "/work/demo",
            source: "project",
            cwd: "/work/demo",
            signal: new AbortController().signal,
        });
        const state = getState();
        const session = state.sessions[state.activeSessionId];
        expect(state.windows[session.activeWindowId]).toMatchObject({ transient: true, activePaneId: paneId });
        invoke.mockResolvedValue(undefined);

        await expect(flushPersist()).resolves.toBe(true);
        const raw = invoke.mock.calls[0][1].data as string;
        const saved = JSON.parse(raw);
        const savedSession = saved.sessions.find((candidate: { id: string }) => candidate.id === session.id);
        const savedWindows = saved.windowsBySession[session.id];

        expect(savedWindows.length).toBeGreaterThan(0);
        expect(savedWindows.every((window: { transient?: unknown }) => window.transient === undefined)).toBe(true);
        expect(savedWindows.some((window: { id: string }) => window.id === savedSession.activeWindowId)).toBe(true);
        expect(raw).not.toContain("task:test:/work/demo");
        expect(raw).not.toContain("externalPty");
    });

    it("classifies hydration before persistence can overwrite protected state", () => {
        setState({ themeId: "unchanged" });

        expect(applyHydrate("")).toBe("empty");
        expect(hydrationAllowsPersistence("empty")).toBe(true);
        expect(applyHydrate("{")).toBe("invalid");
        expect(hydrationAllowsPersistence("invalid")).toBe(false);
        expect(
            applyHydrate(
                JSON.stringify({
                    version: 8,
                    sessions: [],
                    itemStates: {},
                }),
            ),
        ).toBe("unsupported-future");
        expect(hydrationAllowsPersistence("unsupported-future")).toBe(false);
        expect(getState().themeId).toBe("unchanged");
        expect(invoke).not.toHaveBeenCalled();
    });

    it("omits Bruno secrets and drafts while preserving non-secret Bruno and Rundeck state", async () => {
        const sid = getState().activeSessionId;
        setState((s) => ({
            sessions: {
                ...s.sessions,
                [sid]: {
                    ...s.sessions[sid],
                    kind: "bruno",
                    bruno: {
                        collectionPath: "/collections/demo",
                        selectedEnvs: { "/collections/demo": "staging" },
                        secretVars: { token: "do-not-persist" },
                        drafts: { "/collections/demo/login.bru": "Authorization: Bearer do-not-persist" },
                    },
                },
            },
            rundeck: { activeProject: "ops", activeEnvFolder: "prod", prodEnvs: ["prod"] },
        }));
        invoke.mockResolvedValue(undefined);

        expect(await flushPersist()).toBe(true);
        const raw = invoke.mock.calls[0][1].data as string;
        expect(raw).not.toContain("do-not-persist");
        const saved = JSON.parse(raw);
        expect(saved.sessions[0].bruno).toEqual({
            collectionPath: "/collections/demo",
            selectedEnvs: { "/collections/demo": "staging" },
        });
        expect(saved.prefs.rundeck).toEqual({ activeProject: "ops", activeEnvFolder: "prod", prodEnvs: ["prod"] });
    });

    it("persists and safely hydrates keybinding overrides", async () => {
        setState({
            keybindingOverrides: {
                "project.open": "Ctrl+KeyP",
                "pane.zoom": null,
            },
        });
        invoke.mockResolvedValue(undefined);

        expect(await flushPersist()).toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);
        expect(saved.prefs.keybindingOverrides).toEqual({
            "project.open": "Ctrl+KeyP",
            "pane.zoom": null,
        });

        applyHydrate(
            JSON.stringify({
                ...saved,
                prefs: {
                    ...saved.prefs,
                    keybindingOverrides: {
                        "project.open": "Meta+Shift+KeyO",
                        "pane.zoom": "KeyZ",
                        unknown: "Meta+KeyU",
                    },
                },
            }),
        );
        expect(getState().keybindingOverrides).toEqual({ "project.open": "Meta+Shift+KeyO" });
    });

    it("persists rail visibility by role", async () => {
        setState({ sideRailOpen: false, workspaceRailOpen: true });
        invoke.mockResolvedValue(undefined);

        await expect(flushPersist()).resolves.toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);
        expect(saved.prefs).toMatchObject({ sideRailOpen: false, workspaceRailOpen: true });
        expect(saved.prefs).not.toHaveProperty("leftRailOpen");
        expect(saved.prefs).not.toHaveProperty("rightRailOpen");

        setState({ sideRailOpen: true, workspaceRailOpen: false });
        applyHydrate(JSON.stringify(saved));
        expect(getState()).toMatchObject({ sideRailOpen: false, workspaceRailOpen: true });
    });

    it("never persists or hydrates live agent commands", async () => {
        const sid = getState().activeSessionId;
        const agent = {
            id: "agent-live",
            type: "claude" as const,
            title: "live",
            startup: "claude --resume should-never-auto-run",
        };
        setState((s) => ({
            sessions: { ...s.sessions, [sid]: { ...s.sessions[sid], kind: "project", view: "agent", activeAgentId: agent.id } },
            agents: { [agent.id]: agent },
            agentsBySession: { ...s.agentsBySession, [sid]: [agent.id] },
        }));
        invoke.mockResolvedValue(undefined);

        expect(await flushPersist()).toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);
        expect(saved.agentsBySession[sid]).toEqual([]);
        expect(saved.sessions[0]).toMatchObject({ view: "windows", activeAgentId: null });
        expect(JSON.stringify(saved)).not.toContain("should-never-auto-run");

        saved.agentsBySession[sid] = [agent];
        saved.sessions[0].view = "agent";
        saved.sessions[0].activeAgentId = agent.id;
        applyHydrate(JSON.stringify(saved));
        expect(getState().agentsBySession[sid]).toEqual([]);
        expect(getState().agents).toEqual({});
        expect(getState().sessions[sid]).toMatchObject({ view: "windows", activeAgentId: null });
    });

    it("restores confirmed agent sessions asleep without trusting saved startup", async () => {
        const sid = getState().activeSessionId;
        const agent = {
            id: "agent-resumable",
            type: "codex" as const,
            title: "fix the parser",
            startup: "malicious saved startup",
            resumeId: "session-123",
            launchState: "live" as const,
            keepAlive: true,
        };
        setState((s) => ({
            sessions: { ...s.sessions, [sid]: { ...s.sessions[sid], kind: "project", view: "agent", activeAgentId: agent.id } },
            agents: { [agent.id]: agent },
            agentsBySession: { ...s.agentsBySession, [sid]: [agent.id] },
        }));
        invoke.mockResolvedValue(undefined);
        expect(await flushPersist()).toBe(true);
        const raw = invoke.mock.calls[0][1].data as string;
        expect(raw).not.toContain("malicious saved startup");
        const saved = JSON.parse(raw);
        expect(saved.agentsBySession[sid]).toEqual([
            { id: agent.id, type: "codex", title: agent.title, resumeId: agent.resumeId, permissionMode: "workspace-write", keepAlive: true },
        ]);

        saved.agentsBySession[sid][0].startup = "still malicious";
        applyHydrate(JSON.stringify(saved));
        const restored = getState().agents[agent.id];
        expect(restored).toMatchObject({ launchState: "dormant", keepAlive: true });
        expect(restored.startup).toMatch(/^codex resume\b/);
        expect(restored.startup).toContain("session-123");
        expect(restored.startup).not.toContain("still malicious");
        expect(getState().sessions[sid]).toMatchObject({ view: "agent", activeAgentId: agent.id });
    });

    it("preserves OMP and Grok reasoning levels across sleep", async () => {
        const sid = getState().activeSessionId;
        const agents = {
            omp: {
                id: "agent-omp",
                type: "omp" as const,
                title: "OMP task",
                startup: "omp",
                resumeId: "/sessions/omp.jsonl",
                effort: "off" as const,
            },
            grok: {
                id: "agent-grok",
                type: "grok" as const,
                title: "Grok task",
                startup: "grok",
                resumeId: "018f0000-0000-7000-8000-000000000000",
                effort: "minimal" as const,
            },
        };
        setState((state) => ({
            sessions: { ...state.sessions, [sid]: { ...state.sessions[sid], kind: "project", view: "agent", activeAgentId: agents.omp.id } },
            agents: { [agents.omp.id]: agents.omp, [agents.grok.id]: agents.grok },
            agentsBySession: { ...state.agentsBySession, [sid]: [agents.omp.id, agents.grok.id] },
        }));
        invoke.mockResolvedValue(undefined);
        expect(await flushPersist()).toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);

        applyHydrate(JSON.stringify(saved));
        expect(getState().agents[agents.omp.id]).toMatchObject({ type: "omp", effort: "off", launchState: "dormant" });
        expect(getState().agents[agents.grok.id]).toMatchObject({ type: "grok", effort: "minimal", launchState: "dormant" });
    });

    it("persists non-secret provider profiles and defensively hydrates selections", async () => {
        setState({
            providerProfiles: [
                {
                    id: "codex-work",
                    name: "Codex Work",
                    provider: "codex",
                    accent: "#ABCDEF",
                    executablePath: "/opt/bin/codex",
                    configPath: "/safe/config.toml",
                    environmentKeys: ["OPENAI_API_KEY", "OPENAI_API_KEY", "bad-key"],
                    apiKey: "must-not-persist",
                } as never,
            ],
            selectedProviderProfileIds: { codex: "codex-work", claude: "missing" },
            defaultAgentPermissionMode: "full-access",
        });
        invoke.mockResolvedValue(undefined);

        expect(await flushPersist()).toBe(true);
        const raw = invoke.mock.calls[0][1].data as string;
        expect(raw).not.toContain("must-not-persist");
        const saved = JSON.parse(raw);
        expect(saved.prefs.providerProfiles).toEqual([
            {
                id: "codex-work",
                name: "Codex Work",
                provider: "codex",
                accent: "#abcdef",
                executablePath: "/opt/bin/codex",
                configPath: "/safe/config.toml",
                environmentKeys: ["OPENAI_API_KEY"],
            },
        ]);
        expect(saved.prefs.selectedProviderProfileIds).toEqual({ codex: "codex-work" });
        expect(saved.prefs.defaultAgentPermissionMode).toBe("workspace-write");

        saved.prefs.providerProfiles.push({ id: "bad", name: "Bad", provider: "unknown", accent: "red", token: "do-not-hydrate" });
        saved.prefs.selectedProviderProfileIds = { codex: "codex-work", claude: "bad", unknown: "codex-work" };
        saved.prefs.defaultAgentPermissionMode = "unbounded";
        applyHydrate(JSON.stringify(saved));
        expect(getState().providerProfiles).toEqual(saved.prefs.providerProfiles.slice(0, 1));
        expect(getState().selectedProviderProfileIds).toEqual({ codex: "codex-work" });
        expect(getState().defaultAgentPermissionMode).toBe("workspace-write");
        expect(JSON.stringify(getState().providerProfiles)).not.toContain("do-not-hydrate");
    });

    it("migrates legacy permission bypass and drops retired worktree metadata", async () => {
        const sid = getState().activeSessionId;
        const session = getState().sessions[sid];
        const legacy = {
            id: "agent-worktree",
            type: "claude" as const,
            title: "isolated task",
            resumeId: "resume-worktree",
            skipPermissions: true,
            profileId: "builtin-claude",
            cwd: "/repo/.worktrees/isolated",
            worktreePath: "/repo/.worktrees/isolated",
        };
        invoke.mockResolvedValue(undefined);
        expect(await flushPersist()).toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);
        saved.sessions[0] = { ...session, kind: "project", view: "agent", activeAgentId: legacy.id };
        saved.agentsBySession[sid] = [legacy];

        applyHydrate(JSON.stringify(saved));
        expect(getState().agents[legacy.id]).toMatchObject({
            permissionMode: "bypass",
            skipPermissions: true,
            profileId: "builtin-claude",
            cwd: legacy.cwd,
            launchState: "dormant",
        });
        expect(getState().agents[legacy.id]).not.toHaveProperty("worktreePath");
        expect(getState().agents[legacy.id].startup).toContain("--dangerously-skip-permissions");
    });

    it("discards a saved agent profile when its provider no longer matches", async () => {
        const sid = getState().activeSessionId;
        const session = getState().sessions[sid];
        invoke.mockResolvedValue(undefined);
        expect(await flushPersist()).toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);
        saved.prefs.providerProfiles = [
            { id: "moved-profile", name: "Now Codex", provider: "codex", accent: "#abcdef", executablePath: "/opt/codex" },
        ];
        saved.sessions[0] = { ...session, kind: "project", view: "agent", activeAgentId: "claude-agent" };
        saved.agentsBySession[sid] = [
            {
                id: "claude-agent",
                type: "claude",
                title: "Claude",
                resumeId: "resume-claude",
                permissionMode: "workspace-write",
                profileId: "moved-profile",
            },
        ];

        applyHydrate(JSON.stringify(saved));

        expect(getState().agents["claude-agent"].profileId).toBeUndefined();
        expect(getState().agents["claude-agent"].startup).toMatch(/^claude /);
    });

    it("writes v7 item envelopes and migrates bounded v6 editor views", async () => {
        const sid = getState().activeSessionId;
        const window = getState().windows[getState().sessions[sid].activeWindowId];
        const editorPane = { type: "pane", id: "editor-v7", cwd: "/repo", kind: "editor", title: "editor" } as const;
        setState((state) => ({
            windows: { ...state.windows, [window.id]: { ...window, root: editorPane, activePaneId: editorPane.id } },
            editorViews: {
                [editorPane.id]: { openTabs: ["/repo/a.ts"], activePath: "/repo/a.ts" },
                orphan: { openTabs: ["/secret"], activePath: "/secret" },
            },
        }));
        invoke.mockResolvedValue(undefined);

        await expect(flushPersist()).resolves.toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);
        expect(saved.version).toBe(7);
        expect(saved.editorViews).toBeUndefined();
        expect(saved.itemStates).toEqual({
            [editorPane.id]: {
                itemId: editorPane.id,
                kind: "editor",
                version: 2,
                state: { openTabs: ["/repo/a.ts"], activePath: "/repo/a.ts" },
            },
        });
        expect(JSON.stringify(saved)).not.toContain("/secret");

        const legacy = {
            ...saved,
            version: 6,
            editorViews: { [editorPane.id]: saved.itemStates[editorPane.id].state },
        };
        delete legacy.itemStates;
        applyHydrate(JSON.stringify(legacy));
        expect(getState().editorViews[editorPane.id]).toEqual(saved.itemStates[editorPane.id].state);
    });

    it("rejects v7 item envelopes with mismatched identity, kind, version, or state", async () => {
        const sid = getState().activeSessionId;
        const window = getState().windows[getState().sessions[sid].activeWindowId];
        const editorPane = { type: "pane", id: "editor-strict", cwd: "/repo", kind: "editor", title: "editor" } as const;
        setState((state) => ({
            windows: { ...state.windows, [window.id]: { ...window, root: editorPane, activePaneId: editorPane.id } },
            editorViews: { [editorPane.id]: { openTabs: [], activePath: null } },
        }));
        invoke.mockResolvedValue(undefined);
        await flushPersist();
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);

        for (const envelope of [
            { ...saved.itemStates[editorPane.id], itemId: "another" },
            { ...saved.itemStates[editorPane.id], kind: "terminal", state: null },
            { ...saved.itemStates[editorPane.id], version: 999 },
            { ...saved.itemStates[editorPane.id], state: { openTabs: ["/a"], activePath: "/missing" } },
        ]) {
            applyHydrate(JSON.stringify({ ...saved, itemStates: { [editorPane.id]: envelope } }));
            expect(getState().editorViews[editorPane.id]).toBeUndefined();
        }
    });

    it("serializes writes, coalesces queued snapshots, and marks only successful writes saved", async () => {
        const first = deferred<void>();
        invoke.mockImplementationOnce(() => first.promise).mockResolvedValue(undefined);

        setState({ themeId: "first" });
        const firstFlush = flushPersist();
        await Promise.resolve();
        expect(invoke).toHaveBeenCalledTimes(1);

        setState({ themeId: "latest" });
        const secondFlush = flushPersist();
        await Promise.resolve();
        expect(invoke).toHaveBeenCalledTimes(1);

        first.resolve();
        await expect(firstFlush).resolves.toBe(true);
        await expect(secondFlush).resolves.toBe(true);
        expect(invoke).toHaveBeenCalledTimes(2);
        expect(JSON.parse(invoke.mock.calls[1][1].data).prefs.themeId).toBe("latest");
    });

    it("drops an obsolete queued snapshot when state returns to the active write", async () => {
        const first = deferred<void>();
        invoke.mockImplementationOnce(() => first.promise).mockResolvedValue(undefined);

        setState({ themeId: "active" });
        const activeFlush = flushPersist();
        await Promise.resolve();
        setState({ themeId: "obsolete" });
        void flushPersist();
        setState({ themeId: "active" });
        void flushPersist();

        first.resolve();
        await expect(activeFlush).resolves.toBe(true);
        expect(invoke).toHaveBeenCalledTimes(1);
    });

    it("surfaces failed saves and retries the unsaved snapshot", async () => {
        vi.useFakeTimers();
        invoke.mockRejectedValueOnce(new Error("disk full")).mockResolvedValue(undefined);
        setState({ themeId: "retry-me" });

        await expect(flushPersist()).resolves.toBe(false);
        expect(useToasts.getState().toasts.at(-1)?.text).toContain("disk full");
        await vi.advanceTimersByTimeAsync(1600);
        expect(invoke).toHaveBeenCalledTimes(2);
        expect(JSON.parse(invoke.mock.calls[1][1].data).prefs.themeId).toBe("retry-me");
    });

    it("hydrates supported old snapshots safely, scrubs legacy Bruno credentials, and ignores malformed records", () => {
        const sid = getState().activeSessionId;
        const current = getState().sessions[sid];
        applyHydrate(
            JSON.stringify({
                version: 3,
                sessions: [
                    {
                        ...current,
                        kind: "bruno",
                        bruno: {
                            collectionPath: "/legacy",
                            selectedEnvs: { "/legacy": "dev" },
                            secretVars: { password: "legacy-secret" },
                            drafts: { "/legacy/a.bru": "legacy-secret" },
                        },
                    },
                    null,
                    { id: 42 },
                ],
                windowsBySession: { [sid]: [] },
                agentsBySession: {},
                sessionOrder: [sid, 42],
                activeSessionId: sid,
                recent: "bad",
                agentBookmarks: null,
                prefs: { rundeck: { activeProject: 7, prodEnvs: ["prod", 9] } },
                editorViews: { bad: { openTabs: "bad" } },
            }),
        );

        expect(getState().sessions[sid].bruno).toEqual({
            collectionPath: "/legacy",
            selectedEnvs: { "/legacy": "dev" },
            secretVars: {},
            drafts: {},
        });
        expect(getState().sessionOrder).toEqual([sid]);
        expect(getState().recent).toEqual([]);
        expect(getState().rundeck.activeProject).toBe("");
        expect(getState().rundeck.prodEnvs).toEqual(["prod"]);

        invoke.mockResolvedValue(undefined);
        const unsubscribe = subscribePersist();
        unsubscribe();
        expect(invoke).toHaveBeenCalledTimes(1);
        const migrated = invoke.mock.calls[0][1].data as string;
        expect(migrated).not.toContain("legacy-secret");
        expect(migrated).not.toContain("agentBookmarks");
        expect(JSON.parse(migrated).version).toBe(7);
    });

    it("upgrades saved SSH terminals to the reconnecting startup command", () => {
        const sid = getState().activeSessionId;
        const session = getState().sessions[sid];
        const window = getState().windows[session.activeWindowId];

        applyHydrate(
            JSON.stringify({
                version: 4,
                sessions: [{ ...session, kind: "ssh", name: "prod-db" }],
                windowsBySession: {
                    [sid]: [
                        {
                            ...window,
                            root: { ...window.root, startup: "ssh prod-db" },
                        },
                    ],
                },
                agentsBySession: {},
                sessionOrder: [sid],
                activeSessionId: sid,
                prefs: {},
            }),
        );

        const restored = getState().windows[session.activeWindowId].root;
        expect(restored.type).toBe("pane");
        if (restored.type === "pane") {
            expect(restored.startup).toContain("Retrying (%s/5)");
            expect(restored.startup).not.toMatch(/[\r\n]/);
        }
    });

    it("replaces the multiline SSH startup from the first reconnect release", () => {
        const sid = getState().activeSessionId;
        const session = getState().sessions[sid];
        const window = getState().windows[session.activeWindowId];

        applyHydrate(
            JSON.stringify({
                version: 4,
                sessions: [{ ...session, kind: "ssh", name: "prod-db" }],
                windowsBySession: {
                    [sid]: [
                        {
                            ...window,
                            root: { ...window.root, startup: "(\n  sikemux_ssh_retries=0\n)" },
                        },
                    ],
                },
                agentsBySession: {},
                sessionOrder: [sid],
                activeSessionId: sid,
                prefs: {},
            }),
        );

        const restored = getState().windows[session.activeWindowId].root;
        expect(restored.type).toBe("pane");
        if (restored.type === "pane") expect(restored.startup).not.toMatch(/[\r\n]/);
    });

    it("upgrades legacy fixed project terminals to regular numbered tabs", () => {
        const sid = getState().activeSessionId;
        const session = getState().sessions[sid];
        const window = getState().windows[session.activeWindowId];

        applyHydrate(
            JSON.stringify({
                version: 4,
                sessions: [{ ...session, kind: "project", cwd: "/work/demo" }],
                windowsBySession: {
                    [sid]: [{ ...window, name: "term", role: "term", fixed: true }],
                },
                agentsBySession: {},
                sessionOrder: [sid],
                activeSessionId: sid,
                prefs: {},
            }),
        );

        const restored = getState().windows[window.id];
        expect(restored).toMatchObject({ name: "1", role: "term" });
        expect(restored.fixed).toBeUndefined();
    });
});
