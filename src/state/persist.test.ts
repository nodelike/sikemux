import "../plugins/builtin";
import { AWS_CONSOLE } from "../plugins/aws/kinds";
import { RUNDECK_DEPLOY } from "../plugins/rundeck/kinds";
import { rundeckSettings } from "../plugins/rundeck/state";
import { beforeEach, describe, expect, it, vi } from "vitest";

const { invoke } = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke }));

import { applyHydrate, flushPersist, hydrationAllowsPersistence, resetPersistenceForTests, subscribePersist } from "./persist";
import * as cmd from "./commands";
import { flushBrunoDrafts, setBrunoDraft, setBrunoSecret } from "../plugins/bruno/runtime";
import { brunoSettings } from "../plugins/bruno/state";
import { basename } from "../lib/paths";
import { BRUNO_CLIENT } from "../plugins/bruno/kinds";
import { getState, setState } from "./store";
import { activeAgentId, agentIdsOf, agentWindowId } from "./selectors";
import { collectPanes } from "./layout";
import { agentWindow } from "./agentWindow";
import { withAgents } from "../test/agents";
import type { Agent } from "./types";
import { useToasts } from "./toast";

function browserTab(id: string, url: string, title: string) {
    return { id, title, url, active: false, loading: false, canGoBack: false, canGoForward: false, favicon: null, acting: false };
}

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
    it("saves durable changes while terminal activity continues", async () => {
        vi.useFakeTimers();
        invoke.mockResolvedValue(undefined);
        const unsubscribe = subscribePersist();
        await vi.advanceTimersByTimeAsync(0);
        invoke.mockClear();
        setState({ uiTextScale: 1.2 });
        for (let index = 0; index < 6; index++) {
            await vi.advanceTimersByTimeAsync(100);
            setState({ terminalTitles: { pane: `output-${index}` } });
        }
        expect(invoke).toHaveBeenCalledTimes(1);
        expect(JSON.parse(invoke.mock.calls[0][1].data).prefs.uiTextScale).toBe(1.2);
        await vi.advanceTimersByTimeAsync(600);
        expect(invoke).toHaveBeenCalledTimes(1);
        unsubscribe();
    });

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
                    version: 16,
                    sessions: [],
                    itemStates: {},
                }),
            ),
        ).toBe("unsupported-future");
        expect(hydrationAllowsPersistence("unsupported-future")).toBe(false);
        expect(getState().themeId).toBe("unchanged");
        expect(invoke).not.toHaveBeenCalled();
    });

    it("omits Bruno secrets and drafts while preserving its settings and other plugins'", async () => {
        const brunoState = {
            collectionPath: "/collections/demo",
            selectedEnvs: { "/collections/demo": "staging" },
            workspaces: ["/collections/demo"],
        };
        setState({
            pluginSettings: {
                "sikemux.bruno": brunoState,
                "sikemux.rundeck": { activeProject: "ops", activeEnvFolder: "prod", prodEnvs: ["prod"] },
            },
        });
        // These live outside the persisted store entirely; the snapshot must come back without them.
        setBrunoSecret("pane-bruno", "token", "do-not-persist");
        setBrunoDraft("pane-bruno", "/collections/demo/login.bru", "Authorization: Bearer do-not-persist");
        flushBrunoDrafts();
        invoke.mockResolvedValue(undefined);

        expect(await flushPersist()).toBe(true);
        const raw = invoke.mock.calls[0][1].data as string;
        expect(raw).not.toContain("do-not-persist");
        const saved = JSON.parse(raw);
        expect(saved.prefs.pluginSettings).toEqual({
            "sikemux.bruno": brunoState,
            "sikemux.rundeck": { activeProject: "ops", activeEnvFolder: "prod", prodEnvs: ["prod"] },
        });
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
        setState({ sideRailOpen: false, agentRailOpen: true });
        invoke.mockResolvedValue(undefined);

        await expect(flushPersist()).resolves.toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);
        expect(saved.prefs).toMatchObject({ sideRailOpen: false, agentRailOpen: true });
        expect(saved.prefs).not.toHaveProperty("leftRailOpen");
        expect(saved.prefs).not.toHaveProperty("rightRailOpen");

        setState({ sideRailOpen: true, agentRailOpen: false });
        applyHydrate(JSON.stringify(saved));
        expect(getState()).toMatchObject({ sideRailOpen: false, agentRailOpen: true });
    });

    it("persists rail widths and pulls stored ones back inside their bounds", async () => {
        setState({ sideRailWidth: 320, agentRailWidth: 400 });
        invoke.mockResolvedValue(undefined);

        await expect(flushPersist()).resolves.toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);
        expect(saved.prefs).toMatchObject({ sideRailWidth: 320, agentRailWidth: 400 });

        saved.prefs.sideRailWidth = 20;
        saved.prefs.agentRailWidth = 9000;
        applyHydrate(JSON.stringify(saved));
        expect(getState()).toMatchObject({ sideRailWidth: 180, agentRailWidth: 560 });
    });

    it("never persists or hydrates live agent commands", async () => {
        const sid = getState().activeSessionId;
        const terminalWindowId = getState().sessions[sid].activeWindowId;
        const agent: Agent = { id: "agent-live", type: "claude", title: "live", startup: "claude --resume should-never-auto-run" };
        setState((s) => {
            const slices = withAgents(s, sid, [agent]);
            return {
                ...slices,
                sessions: { ...s.sessions, [sid]: { ...s.sessions[sid], kind: "project", activeWindowId: agentWindowId(slices, agent.id)! } },
            };
        });
        invoke.mockResolvedValue(undefined);

        expect(await flushPersist()).toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);
        // With no resume id there is nothing to come back to, so neither the
        // agent nor its window is written, and the session falls back to a
        // window that is.
        expect(saved.agents).toEqual([]);
        expect(saved.windowsBySession[sid].map((w: { role: string }) => w.role)).not.toContain("agent");
        expect(saved.sessions[0].activeWindowId).toBe(terminalWindowId);
        expect(JSON.stringify(saved)).not.toContain("should-never-auto-run");

        saved.agents = [agent];
        saved.windowsBySession[sid].push(agentWindow(agent, "/repo"));
        applyHydrate(JSON.stringify(saved));
        expect(agentIdsOf(getState(), sid)).toEqual([]);
        expect(getState().agents).toEqual({});
        expect(getState().windows[getState().sessions[sid].activeWindowId].role).not.toBe("agent");
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
        setState((s) => {
            const slices = withAgents(s, sid, [agent]);
            return {
                ...slices,
                sessions: { ...s.sessions, [sid]: { ...s.sessions[sid], kind: "project", activeWindowId: agentWindowId(slices, agent.id)! } },
            };
        });
        invoke.mockResolvedValue(undefined);
        expect(await flushPersist()).toBe(true);
        const raw = invoke.mock.calls[0][1].data as string;
        expect(raw).not.toContain("malicious saved startup");
        const saved = JSON.parse(raw);
        expect(saved.agents).toEqual([
            { id: agent.id, type: "codex", title: agent.title, resumeId: agent.resumeId, permissionMode: "workspace-write", keepAlive: true },
        ]);
        expect(saved.windowsBySession[sid].map((w: { role: string }) => w.role)).toContain("agent");

        saved.agents[0].startup = "still malicious";
        applyHydrate(JSON.stringify(saved));
        const restored = getState().agents[agent.id];
        expect(restored).toMatchObject({ launchState: "dormant", keepAlive: true });
        expect(restored.startup).toMatch(/^codex resume\b/);
        expect(restored.startup).toContain("session-123");
        expect(restored.startup).not.toContain("still malicious");
        expect(activeAgentId(getState(), getState().sessions[sid])).toBe(agent.id);
    });

    /* A browser pane is a leaf like any other, but the browser behind it dies
       with the app, so what comes back is the pane plus the pages it held. */
    it("saves an agent's browser tabs and hands them back to the pane that was showing them", async () => {
        const sid = getState().activeSessionId;
        const agent: Agent = { id: "agent-browsing", type: "claude", title: "reading docs", startup: "claude", resumeId: "session-7" };
        setState((s) => {
            const slices = withAgents(s, sid, [agent]);
            return {
                ...slices,
                sessions: { ...s.sessions, [sid]: { ...s.sessions[sid], kind: "project", activeWindowId: agentWindowId(slices, agent.id)! } },
            };
        });
        cmd.openBrowserPane(agent.id);
        const windowId = agentWindowId(getState(), agent.id)!;
        const paneId = collectPanes(getState().windows[windowId].root).find((pane) => pane.kind === "browser")!.id;
        setState({
            browserStrips: {
                [agent.id]: {
                    tabs: [browserTab("tab-blank", "about:blank", ""), browserTab("tab-docs", "https://example.com/docs", "Docs")],
                    activeTabId: "tab-docs",
                },
            },
        } as never);
        invoke.mockResolvedValue(undefined);

        expect(await flushPersist()).toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);
        const savedWindow = saved.windowsBySession[sid].find((w: { id: string }) => w.id === windowId);

        expect(collectPanes(savedWindow.root).map((pane) => pane.kind)).toEqual(["agent", "browser"]);
        expect(saved.itemStates[paneId]).toEqual({
            itemId: paneId,
            kind: "browser",
            version: 1,
            state: { agentId: agent.id, tabs: [{ url: "https://example.com/docs", title: "Docs" }], activeIndex: 0 },
        });

        applyHydrate(JSON.stringify(saved));

        expect(getState().browserPanes[paneId]).toBe(agent.id);
        expect(getState().browserRestores[paneId].tabs).toEqual([{ url: "https://example.com/docs", title: "Docs" }]);
        expect(getState().browserStrips).toEqual({});
    });

    it("drops a browser pane with no page left to open, and one whose agent did not come back", async () => {
        const sid = getState().activeSessionId;
        const agent: Agent = { id: "agent-browsing", type: "claude", title: "reading docs", startup: "claude", resumeId: "session-7" };
        setState((s) => {
            const slices = withAgents(s, sid, [agent]);
            return {
                ...slices,
                sessions: { ...s.sessions, [sid]: { ...s.sessions[sid], kind: "project", activeWindowId: agentWindowId(slices, agent.id)! } },
            };
        });
        cmd.openBrowserPane(agent.id);
        const windowId = agentWindowId(getState(), agent.id)!;
        invoke.mockResolvedValue(undefined);

        expect(await flushPersist()).toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);
        const savedWindow = saved.windowsBySession[sid].find((w: { id: string }) => w.id === windowId);
        expect(savedWindow.root).toMatchObject({ type: "pane", kind: "agent", id: agent.id });
        expect(savedWindow.activePaneId).toBe(agent.id);

        // The same window, saved with its tabs, but read back without the agent.
        const paneId = "pane-browser-orphan";
        savedWindow.root = {
            type: "split",
            id: "split-restored",
            dir: "row",
            sizes: [0.5, 0.5],
            children: [
                { type: "pane", id: agent.id, cwd: "/repo", kind: "agent", title: agent.title },
                { type: "pane", id: paneId, cwd: "/repo", kind: "browser", title: "browser" },
            ],
        };
        saved.itemStates[paneId] = {
            itemId: paneId,
            kind: "browser",
            version: 1,
            state: { agentId: "agent-that-is-gone", tabs: [{ url: "https://example.com", title: "Example" }], activeIndex: 0 },
        };
        applyHydrate(JSON.stringify(saved));

        expect(getState().browserPanes).toEqual({});
        expect(getState().browserRestores).toEqual({});
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
            sessions: { ...state.sessions, [sid]: { ...state.sessions[sid], kind: "project" } },
            ...withAgents(state, sid, [agents.omp, agents.grok]),
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

    it("adopts the current default safety boundary for snapshots saved before the migration", async () => {
        setState({ defaultAgentPermissionMode: "bypass" });
        invoke.mockResolvedValue(undefined);
        expect(await flushPersist()).toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);

        saved.version = 8;
        saved.prefs.defaultAgentPermissionMode = "workspace-write";
        setState({ defaultAgentPermissionMode: "bypass" });
        applyHydrate(JSON.stringify(saved));
        expect(getState().defaultAgentPermissionMode).toBe("bypass");

        saved.version = 9;
        applyHydrate(JSON.stringify(saved));
        expect(getState().defaultAgentPermissionMode).toBe("workspace-write");
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
        saved.version = 7;
        saved.sessions[0] = { ...session, kind: "project", view: "agent", activeAgentId: legacy.id };
        saved.agentsBySession = { [sid]: [legacy] };

        applyHydrate(JSON.stringify(saved));
        expect(activeAgentId(getState(), getState().sessions[sid])).toBe(legacy.id);
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
        saved.sessions[0] = { ...session, kind: "project" };
        saved.agents = [
            {
                id: "claude-agent",
                type: "claude",
                title: "Claude",
                resumeId: "resume-claude",
                permissionMode: "workspace-write",
                profileId: "moved-profile",
            },
        ];
        saved.windowsBySession[sid].push(agentWindow({ id: "claude-agent", title: "Claude" }, "/repo"));

        applyHydrate(JSON.stringify(saved));

        expect(getState().agents["claude-agent"].profileId).toBeUndefined();
        expect(getState().agents["claude-agent"].startup).toMatch(/^claude /);
    });

    it("writes item envelopes and migrates bounded v6 editor views", async () => {
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
        expect(saved.version).toBe(15);
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

        expect(getState().sessions[sid].kind).toBe(BRUNO_CLIENT);
        expect(brunoSettings.get()).toEqual({ collectionPath: "/legacy", selectedEnvs: { "/legacy": "dev" }, workspaces: ["/legacy"] });
        expect(getState().sessionOrder).toEqual([sid]);
        expect(getState().recent).toEqual([]);
        expect(rundeckSettings.get().activeProject).toBe("");
        expect(rundeckSettings.get().prodEnvs).toEqual(["prod"]);

        invoke.mockResolvedValue(undefined);
        const unsubscribe = subscribePersist();
        unsubscribe();
        expect(invoke).toHaveBeenCalledTimes(1);
        const migrated = invoke.mock.calls[0][1].data as string;
        expect(migrated).not.toContain("legacy-secret");
        expect(migrated).not.toContain("agentBookmarks");
        expect(JSON.parse(migrated).version).toBe(15);
    });

    /*
     * Before v8 an agent sat beside its session and the session named the one it
     * was looking at. Each becomes a window, and that one becomes the active window.
     */
    it("migrates v7 agents into windows and keeps the one being looked at active", async () => {
        const sid = getState().activeSessionId;
        const session = getState().sessions[sid];
        const window = getState().windows[session.activeWindowId];
        const legacyAgent = (id: string) => ({ id, type: "codex", title: `task ${id}`, resumeId: `resume-${id}`, permissionMode: "workspace-write" });
        applyHydrate(
            JSON.stringify({
                version: 7,
                sessions: [{ ...session, kind: "project", cwd: "/repo", view: "agent", activeAgentId: "a2" }],
                windowsBySession: { [sid]: [window] },
                agentsBySession: { [sid]: [legacyAgent("a1"), legacyAgent("a2")] },
                sessionOrder: [sid],
                activeSessionId: sid,
                prefs: {},
                itemStates: {},
            }),
        );

        expect(agentIdsOf(getState(), sid)).toEqual(["a1", "a2"]);
        expect(activeAgentId(getState(), getState().sessions[sid])).toBe("a2");
        expect(getState().windowsBySession[sid][0]).toBe(window.id);
        expect(getState().agents.a1).toMatchObject({ launchState: "dormant", resumeId: "resume-a1" });

        invoke.mockResolvedValue(undefined);
        expect(await flushPersist()).toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);
        expect(saved.version).toBe(15);
        expect(saved.agents.map((agent: { id: string }) => agent.id)).toEqual(["a1", "a2"]);
        expect(saved).not.toHaveProperty("agentsBySession");
        expect(saved.sessions[0]).not.toHaveProperty("view");
    });

    it("moves v9 Rundeck sessions, windows, panes and command contexts onto the plugin's kind", () => {
        const project = getState().sessions[getState().activeSessionId];
        const rundeckPane = { type: "pane", id: "pane-rundeck", cwd: "", kind: "rundeck", title: "rundeck" };
        applyHydrate(
            JSON.stringify({
                version: 9,
                sessions: [{ ...project, id: "s-rundeck", name: "rundeck", kind: "rundeck", cwd: "", activeWindowId: "w-rundeck" }],
                windowsBySession: {
                    "s-rundeck": [
                        { id: "w-rundeck", name: "rundeck", role: "rundeck", root: rundeckPane, activePaneId: "pane-rundeck", fixed: true },
                    ],
                },
                sessionOrder: ["s-rundeck"],
                activeSessionId: "s-rundeck",
                prefs: {
                    customCommands: [
                        { id: "deploy", title: "Deploy", detail: "", command: "rnd run", contexts: ["rundeck", "project"], placement: "terminal" },
                    ],
                },
                itemStates: {},
            }),
        );

        const st = getState();
        expect(st.sessions["s-rundeck"].kind).toBe(RUNDECK_DEPLOY);
        expect(st.windows["w-rundeck"].role).toBe(RUNDECK_DEPLOY);
        expect(st.windows["w-rundeck"].root).toMatchObject({ type: "pane", kind: RUNDECK_DEPLOY });
        expect(st.customCommands[0].contexts).toEqual([RUNDECK_DEPLOY, "project"]);
    });

    it("moves v10 Rundeck settings and each session's deploy location into the plugin's settings", () => {
        const project = getState().sessions[getState().activeSessionId];
        const window = getState().windows[project.activeWindowId];
        applyHydrate(
            JSON.stringify({
                version: 10,
                sessions: [{ ...project, kind: "project", cwd: "/repo/api", deploy: { project: "channeliq", folder: "production" } }],
                windowsBySession: { [project.id]: [window] },
                sessionOrder: [project.id],
                activeSessionId: project.id,
                prefs: { rundeck: { activeProject: "channeliq", activeEnvFolder: "dev", prodEnvs: ["prod"] } },
                itemStates: {},
            }),
        );

        expect(rundeckSettings.get()).toEqual({
            activeProject: "channeliq",
            activeGroup: "dev",
            prodEnvs: ["prod"],
            branchOptions: ["BRANCH", "GIT_BRANCH", "GIT_REF", "REF"],
            deployTargets: {},
            treeHidden: false,
        });
        expect(getState().sessions[project.id]).not.toHaveProperty("deploy");
    });

    it("turns v14 Rundeck env folders into group paths and drops folder-based deploy picks", () => {
        const project = getState().sessions[getState().activeSessionId];
        const window = getState().windows[project.activeWindowId];
        applyHydrate(
            JSON.stringify({
                version: 14,
                sessions: [{ ...project, kind: "project" }],
                windowsBySession: { [project.id]: [window] },
                sessionOrder: [project.id],
                activeSessionId: project.id,
                prefs: {
                    pluginSettings: {
                        "sikemux.rundeck": {
                            activeProject: "ops",
                            activeEnvFolder: "Prod",
                            prodEnvs: ["prod", "live"],
                            deployTargets: { "/repo/api": { project: "ops", folder: "Prod" } },
                        },
                    },
                },
                itemStates: {},
            }),
        );

        expect(rundeckSettings.get()).toMatchObject({ activeProject: "ops", activeGroup: "Prod", prodEnvs: ["prod", "live"], deployTargets: {} });
    });

    it("folds v11 Bruno sessions, one per workspace, into the one Bruno session and keeps every folder", () => {
        const project = getState().sessions[getState().activeSessionId];
        const window = getState().windows[project.activeWindowId];
        const legacy = (id: string, folder: string) => ({
            ...project,
            id,
            name: basename(folder),
            kind: "bruno",
            cwd: folder,
            bruno: { collectionPath: folder, selectedEnvs: {} },
        });
        applyHydrate(
            JSON.stringify({
                version: 11,
                sessions: [legacy("bruno-1", "/ws/api-docs"), legacy("bruno-2", "/ws/billing")],
                windowsBySession: {
                    "bruno-1": [{ ...window, id: "w-bruno-1", name: "bruno", role: "bruno" }],
                    "bruno-2": [{ ...window, id: "w-bruno-2", name: "bruno", role: "bruno" }],
                },
                sessionOrder: ["bruno-1", "bruno-2"],
                activeSessionId: "bruno-2",
                prefs: { brunoWorkspaces: ["/ws/old"] },
                itemStates: {},
            }),
        );

        const st = getState();
        expect(Object.values(st.sessions).map((session) => [session.kind, session.name])).toEqual([[BRUNO_CLIENT, "Bruno"]]);
        expect(st.activeSessionId).toBe("bruno-1");
        expect(brunoSettings.get().collectionPath).toBe("/ws/api-docs");
        expect(brunoSettings.get().workspaces).toEqual(expect.arrayContaining(["/ws/old", "/ws/api-docs", "/ws/billing"]));
    });

    it("names one-of-a-kind sessions after their tools whatever name was saved", () => {
        cmd.openPluginSession(BRUNO_CLIENT);
        const bruno = getState().sessions[getState().activeSessionId];
        const brunoWindow = getState().windows[bruno.activeWindowId];
        cmd.openPluginSession(AWS_CONSOLE);
        const aws = getState().sessions[getState().activeSessionId];
        const awsWindow = getState().windows[aws.activeWindowId];
        applyHydrate(
            JSON.stringify({
                version: 14,
                sessions: [
                    { ...bruno, name: "bruno" },
                    { ...aws, name: "aws" },
                ],
                windowsBySession: { [bruno.id]: [brunoWindow], [aws.id]: [awsWindow] },
                sessionOrder: [bruno.id, aws.id],
                activeSessionId: bruno.id,
                prefs: {},
                itemStates: {},
            }),
        );

        expect(getState().sessions[bruno.id].name).toBe("Bruno");
        expect(getState().sessions[aws.id].name).toBe("AWS");
    });

    it("moves a v13 Bruno session, workspaces and shortcuts into the Bruno plugin", () => {
        const project = getState().sessions[getState().activeSessionId];
        const window = getState().windows[project.activeWindowId];
        const legacyRoot = { ...window.root, kind: "bruno" };
        applyHydrate(
            JSON.stringify({
                version: 13,
                sessions: [
                    {
                        ...project,
                        id: "s-bruno",
                        name: "Bruno",
                        kind: "bruno",
                        cwd: "/ws/api-docs",
                        activeWindowId: "w-bruno",
                        bruno: { collectionPath: "/ws/api-docs", selectedEnvs: { "/ws/api-docs": "staging" } },
                    },
                ],
                windowsBySession: { "s-bruno": [{ ...window, id: "w-bruno", name: "bruno", role: "bruno", root: legacyRoot }] },
                sessionOrder: ["s-bruno"],
                activeSessionId: "s-bruno",
                prefs: {
                    brunoWorkspaces: ["/ws/api-docs", "/ws/billing"],
                    keybindingOverrides: { "bruno.send": "Alt+Enter", "bruno.open": null, "pane.zoom": "Alt+KeyZ" },
                    customCommands: [{ id: "c1", title: "curl", command: "curl", contexts: ["bruno"], placement: "terminal" }],
                },
                itemStates: {},
            }),
        );

        const st = getState();
        expect(st.sessions["s-bruno"]).toMatchObject({ kind: BRUNO_CLIENT, name: "Bruno" });
        expect(st.sessions["s-bruno"]).not.toHaveProperty("bruno");
        expect(st.windows["w-bruno"]).toMatchObject({ role: BRUNO_CLIENT, root: { type: "pane", kind: BRUNO_CLIENT } });
        expect(brunoSettings.get()).toEqual({
            collectionPath: "/ws/api-docs",
            selectedEnvs: { "/ws/api-docs": "staging" },
            workspaces: ["/ws/api-docs", "/ws/billing"],
        });
        expect(st.keybindingOverrides).toEqual({
            "plugin.run:sikemux.bruno/send": "Alt+Enter",
            "plugin.open:sikemux.bruno": null,
            "pane.zoom": "Alt+KeyZ",
        });
        expect(st.customCommands[0]?.contexts).toEqual([BRUNO_CLIENT]);
    });

    it("moves v12 AWS sessions, settings and shortcut into the AWS plugin", () => {
        cmd.openPluginSession(AWS_CONSOLE);
        const aws = getState().sessions[getState().activeSessionId];
        const awsWindow = getState().windows[aws.activeWindowId];
        const legacyRoot = { ...awsWindow.root, kind: "aws" };
        applyHydrate(
            JSON.stringify({
                version: 12,
                sessions: [{ ...aws, kind: "aws", name: "aws" }],
                windowsBySession: { [aws.id]: [{ ...awsWindow, role: "aws", root: legacyRoot }] },
                sessionOrder: [aws.id],
                activeSessionId: aws.id,
                prefs: { awsProfile: "prod-admin", awsService: "billing", keybindingOverrides: { "aws.open": "Alt+Shift+KeyA" } },
                itemStates: {},
            }),
        );

        const st = getState();
        expect(st.sessions[aws.id]).toMatchObject({ kind: AWS_CONSOLE, name: "AWS" });
        expect(st.windows[awsWindow.id]).toMatchObject({ role: AWS_CONSOLE, root: { type: "pane", kind: AWS_CONSOLE } });
        expect(st.pluginSettings["sikemux.aws"]).toEqual({ profile: "prod-admin", service: "billing" });
        expect(st.keybindingOverrides).toEqual({ "plugin.open:sikemux.aws": "Alt+Shift+KeyA" });
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
