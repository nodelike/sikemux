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
import { deskEditorId } from "./desks";
import { withAgents } from "../test/agents";
import type { Workspace } from "../api/generated/wire";
import { localWorkspace } from "./coreWorkspace";
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

/** The workspace the core keeps for what the store holds now, which a restart takes back. */
function shared(changes: Partial<Workspace> = {}): Workspace {
    return { ...localWorkspace(getState()), launchers: [], palette: {}, backdrop: { texture: false, image: null }, ...changes };
}

/** Makes the open session a project in /repo holding `agents`, and shows the first. */
function projectWith(agents: Agent[]): string {
    const sid = getState().activeSessionId;
    setState((s) => {
        const slices = withAgents(s, sid, agents);
        const front = agents[0] ? agentWindowId(slices, agents[0].id)! : s.sessions[sid].activeWindowId;
        return { ...slices, sessions: { ...s.sessions, [sid]: { ...s.sessions[sid], kind: "project", cwd: "/repo", activeWindowId: front } } };
    });
    return sid;
}

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
        const paneId = cmd.openTaskTerminal({ terminalKey: "task:test:/work/demo", label: "Test", project: "/work/demo", cwd: "/work/demo" });
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

    it("takes pinned chats from the core across a restart, and drops ones it cannot read", async () => {
        invoke.mockResolvedValue(undefined);
        cmd.pinChat({ agent: "claude", id: "s-1", title: "Release checklist", project: "/work/demo" });
        cmd.pinChat({ agent: "claude", id: "s-1", title: "Again", project: "/work/demo" });
        await expect(flushPersist()).resolves.toBe(true);
        const saved = JSON.parse(invoke.mock.calls.at(-1)![1].data as string);
        expect(saved.prefs).not.toHaveProperty("pinnedChats");
        const workspace = shared();
        expect(workspace.pinned).toEqual([{ provider: "claude", sessionId: "s-1", title: "Release checklist", project: "/work/demo" }]);

        setState({ pinnedChats: [] });
        workspace.pinned.push({ provider: "nobody", sessionId: "x", title: "x", project: "/x" });
        expect(applyHydrate(JSON.stringify(saved), undefined, workspace)).toBe("applied");
        expect(getState().pinnedChats).toEqual([{ agent: "claude", id: "s-1", title: "Release checklist", project: "/work/demo" }]);

        cmd.retitlePinnedChat("claude", "s-1", "Release");
        cmd.unpinChat("claude", "missing");
        expect(getState().pinnedChats.map((pin) => pin.title)).toEqual(["Release"]);
        cmd.unpinChat("claude", "s-1");
        expect(getState().pinnedChats).toEqual([]);
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
                    version: 22,
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
        setState({ sideRailOpen: false, tabRailOpen: true });
        invoke.mockResolvedValue(undefined);

        await expect(flushPersist()).resolves.toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);
        expect(saved.prefs).toMatchObject({ sideRailOpen: false, tabRailOpen: true });
        expect(saved.prefs).not.toHaveProperty("leftRailOpen");
        expect(saved.prefs).not.toHaveProperty("rightRailOpen");

        setState({ sideRailOpen: true, tabRailOpen: false });
        applyHydrate(JSON.stringify(saved));
        expect(getState()).toMatchObject({ sideRailOpen: false, tabRailOpen: true });
    });

    it("remembers which projects may start language servers", async () => {
        setState({ languageServerTrust: { "/trusted": true, "/refused": false } });
        invoke.mockResolvedValue(undefined);

        await expect(flushPersist()).resolves.toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);
        expect(saved.prefs.languageServerTrust).toEqual({ "/trusted": true, "/refused": false });

        setState({ languageServerTrust: {} });
        saved.prefs.languageServerTrust["/odd"] = "yes";
        applyHydrate(JSON.stringify(saved));
        expect(getState().languageServerTrust).toEqual({ "/trusted": true, "/refused": false });
    });

    it("remembers each project's Worktree switch default", async () => {
        setState({ agentWorktreeDefaults: { "/code/app": true } });
        invoke.mockResolvedValue(undefined);

        await expect(flushPersist()).resolves.toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);
        expect(saved.prefs.agentWorktreeDefaults).toEqual({ "/code/app": true });

        setState({ agentWorktreeDefaults: {} });
        saved.prefs.agentWorktreeDefaults["/odd"] = 1;
        applyHydrate(JSON.stringify(saved));
        expect(getState().agentWorktreeDefaults).toEqual({ "/code/app": true });
    });

    it("takes the spaces and each project's space from the core, and remembers which space is shown", async () => {
        const work = cmd.createSpace("Work", "briefcase")!;
        const side = cmd.createSpace("Side projects", "")!;
        cmd.createProjectSession("/office");
        cmd.createProjectSession("/side");
        cmd.setProjectSpace("/office", work);
        cmd.setProjectSpace("/side", side);
        cmd.showSpace(work);
        invoke.mockResolvedValue(undefined);

        await expect(flushPersist()).resolves.toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);
        expect(saved.prefs).not.toHaveProperty("spaces");
        const workspace = shared();
        expect(workspace.spaces).toEqual([
            { id: work, name: "Work", icon: "briefcase" },
            { id: side, name: "Side projects", icon: "" },
        ]);

        setState({ spaces: [], projectSpaces: {}, activeSpaceId: null });
        workspace.spaces[1].icon = "not-an-icon";
        applyHydrate(JSON.stringify(saved), undefined, workspace);

        expect(getState().spaces.map((space) => [space.name, space.icon])).toEqual([
            ["Work", "briefcase"],
            ["Side projects", ""],
        ]);
        expect(getState().projectSpaces).toEqual({ "/office": work, "/side": side });
        expect(getState().activeSpaceId).toBe(work);
    });

    it("keeps the name given to a terminal tab", async () => {
        const state = getState();
        const id = state.windowsBySession[state.activeSessionId].find((winId) => state.windows[winId].role === "term")!;
        cmd.renameWindow(id, "api server");
        invoke.mockResolvedValue(undefined);

        await expect(flushPersist()).resolves.toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);
        cmd.renameWindow(id, "");
        applyHydrate(JSON.stringify(saved));

        expect(getState().windows[id].customName).toBe("api server");
    });

    it("persists rail widths and pulls stored ones back inside their bounds", async () => {
        setState({ sideRailWidth: 320, tabRailWidth: 400 });
        invoke.mockResolvedValue(undefined);

        await expect(flushPersist()).resolves.toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);
        expect(saved.prefs).toMatchObject({ sideRailWidth: 320, tabRailWidth: 400 });

        saved.prefs.sideRailWidth = 20;
        saved.prefs.tabRailWidth = 9000;
        applyHydrate(JSON.stringify(saved));
        expect(getState()).toMatchObject({ sideRailWidth: 180, tabRailWidth: 560 });
    });

    it("restores the core's agents asleep in their saved tabs, building their commands afresh", async () => {
        const agent = {
            id: "agent-resumable",
            type: "codex" as const,
            title: "fix the parser",
            startup: "malicious saved startup",
            resumeId: "session-123",
            launchState: "live" as const,
            keepAlive: true,
            renamed: true,
            worktree: { repo: "/repo", path: "/repo.worktrees/fix", branch: "sikemux/fix", base: "main", startSha: "abc123" },
        };
        const sid = projectWith([agent]);
        invoke.mockResolvedValue(undefined);
        expect(await flushPersist()).toBe(true);
        const raw = invoke.mock.calls[0][1].data as string;
        expect(raw).not.toContain("malicious saved startup");
        const saved = JSON.parse(raw);
        expect(saved).not.toHaveProperty("agents");
        expect(saved.sessions[0]).toEqual({ id: sid, kind: "project", activeWindowId: agentWindowId(getState(), agent.id) });
        expect(saved.windowsBySession[sid].map((w: { role: string }) => w.role)).toContain("agent");
        const workspace = shared();
        expect(workspace.agents).toEqual([
            {
                id: agent.id,
                provider: "codex",
                project: sid,
                cwd: "/repo",
                title: agent.title,
                renamed: true,
                resumeId: agent.resumeId,
                permissionMode: "workspace-write",
                keepAlive: true,
                asleep: false,
                worktree: agent.worktree,
                createdAt: 0,
            },
        ]);

        applyHydrate(JSON.stringify(saved), undefined, {
            ...workspace,
            agents: workspace.agents.map((record) => ({ ...record, startup: "still malicious" })),
        });
        const restored = getState().agents[agent.id];
        expect(restored).toMatchObject({ launchState: "dormant", keepAlive: true, renamed: true, worktree: agent.worktree });
        expect(agentIdsOf(getState(), sid)).toEqual([agent.id]);
        expect(activeAgentId(getState(), getState().sessions[sid])).toBe(agent.id);
    });

    /* A desk is a leaf like any other, but the browser behind it dies with the
       app, so what comes back is the pane plus the pages and files it held. */
    it("saves an agent's pages and files and hands them back to the desk that was showing them", async () => {
        const agent: Agent = { id: "agent-browsing", type: "claude", title: "reading docs", resumeId: "session-7" };
        const sid = projectWith([agent]);
        cmd.openDesk(agent.id);
        const windowId = agentWindowId(getState(), agent.id)!;
        const paneId = collectPanes(getState().windows[windowId].root).find((pane) => pane.kind === "desk")!.id;
        setState({
            browserStrips: {
                [agent.id]: {
                    tabs: [browserTab("tab-blank", "about:blank", ""), browserTab("tab-docs", "https://example.com/docs", "Docs")],
                    activeTabId: "tab-docs",
                },
            },
            editorViews: { ...getState().editorViews, [deskEditorId(agent.id)]: { openTabs: ["/repo/src/a.ts"], activePath: "/repo/src/a.ts" } },
        } as never);
        invoke.mockResolvedValue(undefined);

        expect(await flushPersist()).toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);
        const savedWindow = saved.windowsBySession[sid].find((w: { id: string }) => w.id === windowId);

        expect(collectPanes(savedWindow.root).map((pane) => pane.kind)).toEqual(["agent", "desk"]);
        expect(saved.itemStates[paneId]).toEqual({
            itemId: paneId,
            kind: "desk",
            version: 1,
            state: { agentId: agent.id, tabs: [{ url: "https://example.com/docs", title: "Docs" }], activeIndex: 0, files: ["/repo/src/a.ts"] },
        });

        applyHydrate(JSON.stringify(saved), undefined, shared());

        expect(getState().deskPanes[paneId]).toBe(agent.id);
        expect(getState().deskRestores[paneId].tabs).toEqual([{ url: "https://example.com/docs", title: "Docs" }]);
        expect(getState().editorViews[deskEditorId(agent.id)].openTabs).toEqual(["/repo/src/a.ts"]);
        expect(getState().browserStrips).toEqual({});
    });

    it("brings back a desk that holds only a simulator, showing the device it showed", async () => {
        const sid = getState().activeSessionId;
        const agent: Agent = { id: "agent-phone", type: "claude", title: "testing the app", resumeId: "session-8" };
        setState((s) => {
            const slices = withAgents(s, sid, [agent]);
            return {
                ...slices,
                sessions: { ...s.sessions, [sid]: { ...s.sessions[sid], kind: "project", activeWindowId: agentWindowId(slices, agent.id)! } },
            };
        });
        cmd.openDeskSimulator(agent.id, { device: { udid: "UDID-1", name: "iPhone 17" } });
        const windowId = agentWindowId(getState(), agent.id)!;
        const paneId = collectPanes(getState().windows[windowId].root).find((pane) => pane.kind === "desk")!.id;
        invoke.mockResolvedValue(undefined);

        expect(await flushPersist()).toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);
        expect(saved.itemStates[paneId].state).toEqual({
            agentId: agent.id,
            tabs: [],
            activeIndex: 0,
            files: [],
            simulator: { udid: "UDID-1", name: "iPhone 17" },
        });

        applyHydrate(JSON.stringify(saved));

        expect(getState().deskPanes[paneId]).toBe(agent.id);
        const desk = getState().desks[agent.id];
        expect(desk.simulators).toEqual([expect.objectContaining({ udid: "UDID-1", deviceName: "iPhone 17" })]);
        expect(desk.active).toBe(`simulator:${desk.simulators[0].id}`);
    });

    it("drops a desk with nothing left to open, and one whose agent did not come back", async () => {
        const agent: Agent = { id: "agent-browsing", type: "claude", title: "reading docs", resumeId: "session-7" };
        const sid = projectWith([agent]);
        cmd.openDesk(agent.id);
        const windowId = agentWindowId(getState(), agent.id)!;
        invoke.mockResolvedValue(undefined);

        expect(await flushPersist()).toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);
        const savedWindow = saved.windowsBySession[sid].find((w: { id: string }) => w.id === windowId);
        expect(savedWindow.root).toMatchObject({ type: "pane", kind: "agent", id: agent.id });
        expect(savedWindow.activePaneId).toBe(agent.id);

        // The same window, saved with its tabs, but read back without the agent.
        const paneId = "pane-desk-orphan";
        savedWindow.root = {
            type: "split",
            id: "split-restored",
            dir: "row",
            sizes: [0.5, 0.5],
            children: [
                { type: "pane", id: agent.id, cwd: "/repo", kind: "agent", title: agent.title },
                { type: "pane", id: paneId, cwd: "/repo", kind: "desk", title: "desk" },
            ],
        };
        saved.itemStates[paneId] = {
            itemId: paneId,
            kind: "desk",
            version: 1,
            state: { agentId: "agent-that-is-gone", tabs: [{ url: "https://example.com", title: "Example" }], activeIndex: 0, files: [] },
        };
        applyHydrate(JSON.stringify(saved), undefined, shared());

        expect(getState().deskPanes).toEqual({});
        expect(getState().deskRestores).toEqual({});
    });

    it("saves the core terminal each pane and terminal agent shows, and takes them back", async () => {
        const sid = getState().activeSessionId;
        const terminalWindowId = getState().sessions[sid].activeWindowId;
        const paneId = getState().windows[terminalWindowId].activePaneId;
        cmd.setPanePty(paneId, 1_759_300_000_001);
        const agent: Agent = { id: "agent-tui", type: "pi", title: "pi in its terminal", ptyId: 1_759_300_000_002 };
        setState((s) => {
            const slices = withAgents(s, sid, [agent]);
            return { ...slices, sessions: { ...s.sessions, [sid]: { ...s.sessions[sid], kind: "project", cwd: "/repo" } } };
        });
        invoke.mockResolvedValue(undefined);

        expect(await flushPersist()).toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);
        const workspace = shared();
        expect(workspace.agents).toEqual([expect.objectContaining({ id: agent.id, ptyId: 1_759_300_000_002 })]);
        expect(workspace.agents[0]).not.toHaveProperty("resumeId");

        applyHydrate(JSON.stringify(saved), undefined, workspace);
        expect(collectPanes(getState().windows[terminalWindowId].root)[0].ptyId).toBe(1_759_300_000_001);
        expect(getState().agents[agent.id]).toMatchObject({ ptyId: 1_759_300_000_002, launchState: "dormant" });
        expect(agentIdsOf(getState(), sid)).toEqual([agent.id]);
    });

    it("forgets terminals saved before v18, which died with the app", async () => {
        const sid = getState().activeSessionId;
        const terminalWindowId = getState().sessions[sid].activeWindowId;
        invoke.mockResolvedValue(undefined);
        expect(await flushPersist()).toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);
        saved.version = 17;
        saved.windowsBySession[sid][0].root.ptyId = 42;

        expect(applyHydrate(JSON.stringify(saved))).toBe("applied");
        expect(collectPanes(getState().windows[terminalWindowId].root)[0].ptyId).toBeUndefined();

        saved.version = 18;
        saved.windowsBySession[sid][0].root.ptyId = -1;
        applyHydrate(JSON.stringify(saved));
        expect(getState().windows[terminalWindowId]).toBeUndefined();
    });

    it("closes a v16 GitHub session, which lives in the git pane now", async () => {
        const sid = getState().activeSessionId;
        invoke.mockResolvedValue(undefined);
        expect(await flushPersist()).toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);
        const project = saved.sessions.find((row: { id: string }) => row.id === sid);
        saved.version = 16;
        saved.sessions.push({ ...project, id: "session-github", kind: "sikemux.github:hub", name: "GitHub" });
        saved.sessionOrder = [...(saved.sessionOrder ?? []), "session-github"];
        saved.windowsBySession["session-github"] = [];
        saved.activeSessionId = "session-github";

        expect(applyHydrate(JSON.stringify(saved))).toBe("applied");

        expect(getState().sessions["session-github"]).toBeUndefined();
        expect(getState().sessionOrder).not.toContain("session-github");
        expect(getState().activeSessionId).toBe(sid);
    });

    it("moves a v15 browser pane onto a desk with the pages it held", async () => {
        const agent: Agent = { id: "agent-browsing", type: "claude", title: "reading docs", resumeId: "session-7" };
        const sid = projectWith([agent]);
        const windowId = agentWindowId(getState(), agent.id)!;
        invoke.mockResolvedValue(undefined);
        expect(await flushPersist()).toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);
        const savedWindow = saved.windowsBySession[sid].find((w: { id: string }) => w.id === windowId);
        const paneId = "pane-browser-v15";
        saved.version = 15;
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
            state: { agentId: agent.id, tabs: [{ url: "https://example.com", title: "Example" }], activeIndex: 0 },
        };

        expect(applyHydrate(JSON.stringify(saved), undefined, shared())).toBe("applied");

        expect(collectPanes(getState().windows[windowId].root).map((pane) => pane.kind)).toEqual(["agent", "desk"]);
        expect(getState().deskPanes[paneId]).toBe(agent.id);
        expect(getState().deskRestores[paneId]).toEqual({
            agentId: agent.id,
            tabs: [{ url: "https://example.com", title: "Example" }],
            activeIndex: 0,
            files: [],
        });
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
            sessions: { ...state.sessions, [sid]: { ...state.sessions[sid], kind: "project", cwd: "/repo" } },
            ...withAgents(state, sid, [agents.omp, agents.grok]),
        }));
        invoke.mockResolvedValue(undefined);
        expect(await flushPersist()).toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);

        applyHydrate(JSON.stringify(saved), undefined, shared());
        expect(getState().agents[agents.omp.id]).toMatchObject({ type: "omp", effort: "off", launchState: "dormant" });
        expect(getState().agents[agents.grok.id]).toMatchObject({ type: "grok", effort: "minimal", launchState: "dormant" });
    });

    it("leaves provider profiles to the core and defensively hydrates the ones it keeps", async () => {
        setState({
            providerProfiles: [{ id: "codex-work", name: "Codex Work", provider: "codex", accent: "#abcdef" }],
            selectedProviderProfileIds: { codex: "codex-work" },
            defaultAgentPermissionMode: "bypass",
        });
        invoke.mockResolvedValue(undefined);

        expect(await flushPersist()).toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);
        expect(saved.prefs.providerProfiles).toBeUndefined();
        expect(saved.prefs.selectedProviderProfileIds).toBeUndefined();
        expect(saved.prefs).not.toHaveProperty("defaultAgentPermissionMode");

        const kept = {
            id: "codex-work",
            name: "Codex Work",
            provider: "codex",
            accent: "#ABCDEF",
            executablePath: "/opt/bin/codex",
            configPath: "/safe/config.toml",
            environmentKeys: ["OPENAI_API_KEY", "OPENAI_API_KEY", "bad-key"],
            apiKey: "must-not-hydrate",
        };
        const bad = { id: "bad", name: "Bad", provider: "unknown", accent: "red", token: "do-not-hydrate" };
        applyHydrate(
            JSON.stringify(saved),
            {
                profiles: [kept, bad] as never,
                selected: { codex: "codex-work", claude: "bad", unknown: "codex-work" } as never,
                autoSwitch: {},
            },
            shared({ permissionMode: "unbounded" }),
        );
        expect(getState().providerProfiles).toEqual([
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
        expect(getState().selectedProviderProfileIds).toEqual({ codex: "codex-work" });
        expect(getState().defaultAgentPermissionMode).toBe("workspace-write");
        expect(JSON.stringify(getState().providerProfiles)).not.toContain("hydrate");
    });

    it("discards an agent's profile when its provider no longer matches", async () => {
        const sid = projectWith([]);
        invoke.mockResolvedValue(undefined);
        expect(await flushPersist()).toBe(true);
        const saved = JSON.parse(invoke.mock.calls[0][1].data as string);
        const workspace = shared();
        workspace.agents.push({
            id: "claude-agent",
            provider: "claude",
            project: sid,
            cwd: "/repo",
            title: "Claude",
            renamed: false,
            resumeId: "resume-claude",
            permissionMode: "workspace-write",
            profileId: "moved-profile",
            keepAlive: false,
            asleep: true,
            createdAt: 0,
        });

        applyHydrate(
            JSON.stringify(saved),
            {
                profiles: [{ id: "moved-profile", name: "Now Codex", provider: "codex", accent: "#abcdef", executablePath: "/opt/codex" }],
                selected: {},
                autoSwitch: {},
            },
            workspace,
        );

        expect(getState().agents["claude-agent"].profileId).toBeUndefined();
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
        expect(saved.version).toBe(21);
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
                version: 4,
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

        invoke.mockResolvedValue(undefined);
        const unsubscribe = subscribePersist();
        unsubscribe();
        expect(invoke).toHaveBeenCalledTimes(1);
        const migrated = invoke.mock.calls[0][1].data as string;
        expect(migrated).not.toContain("legacy-secret");
        expect(migrated).not.toContain("agentBookmarks");
        expect(JSON.parse(migrated).version).toBe(21);
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
            undefined,
            shared({ projects: [{ id: project.id, name: "api", path: "/repo/api", pinned: false }] }),
        );

        expect(rundeckSettings.get()).toEqual({
            activeProject: "channeliq",
            activeGroup: "dev",
            deployTargets: {},
            treeHidden: false,
            notifyWhenDone: true,
        });
        expect(getState().sessions[project.id]).not.toHaveProperty("deploy");
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
        }
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
        expect(restored).toMatchObject({ name: "Terminal", role: "term" });
        expect(restored.fixed).toBeUndefined();
    });

    it("makes a legacy fixed Git tab in a project closable", () => {
        const sid = getState().activeSessionId;
        const session = getState().sessions[sid];
        const window = getState().windows[session.activeWindowId];

        applyHydrate(
            JSON.stringify({
                version: 4,
                sessions: [{ ...session, kind: "project", cwd: "/work/demo" }],
                windowsBySession: {
                    [sid]: [{ ...window, name: "git", role: "git", root: { ...window.root, kind: "git" }, fixed: true }],
                },
                agentsBySession: {},
                sessionOrder: [sid],
                activeSessionId: sid,
                prefs: {},
            }),
            undefined,
            shared({ projects: [{ id: sid, name: "demo", path: "/work/demo", pinned: false }] }),
        );

        expect(getState().windows[window.id]).toMatchObject({ role: "git" });
        expect(getState().windows[window.id].fixed).toBeUndefined();
    });
});
