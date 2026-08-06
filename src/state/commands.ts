import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import type { AgentSession } from "../api/agents";
import { awsApi } from "../api/aws";
import { fsapi } from "../api/fs";
import { lsp } from "../api/lsp";
import { sshApi } from "../api/ssh";
import { emptyRequest } from "../bruno/types";
import { parseRequest } from "../bruno/parse";
import { serializeRequest } from "../bruno/serialize";
import { basename, dirname, isPathWithin, joinPath } from "../lib/paths";
import { IS_WINDOWS } from "../lib/platform";
import { cloneTheme, DEFAULT_THEME_ID, THEMES_BY_ID, type Theme } from "../themes";
import { sshStartup } from "../terminal/sshStartup";
import { applyTheme, applyWindowOpacity, previewTheme, registerCustomThemes } from "../themes/bus";
import { emit } from "./bus";
import { reduceAgentState } from "./agentStatus";
import { fetchResource, invalidate, peekResource } from "./resources";
import { agentSessionsR, awsIdentityR, projectRootsScanR } from "./resources.defs";
import { envFolderOf, inferEnv } from "./rundeckShape";
import { getState, mutate, setState, type StoreState } from "./store";
import { notify, reportError, swallow } from "./toast";
import { SKIP_PERMISSION_FLAG, agentSupportsSkipPermissions } from "./commands/agentLogic";
import { parseSessionBundle } from "./sessionBundle";
import { DEFAULT_BRUNO_VIEW, DEFAULT_GIT_VIEW, DEFAULT_GLOBAL_SEARCH_VIEW } from "./types";
import {
    collectPanes,
    cloneLayout,
    computeLayout,
    makePane,
    neighborPane,
    newId,
    removePane,
    replacePane,
    resizeTowards,
    setSplitSizes as setSplitSizesFn,
    splitPane,
} from "./layout";
import type {
    Agent,
    AgentType,
    AwsService,
    BrunoReqTab,
    BrunoResTab,
    BrunoView,
    CliOpenRequest,
    CliOpenResult,
    CliOpenTarget,
    DeployRef,
    EcsLevel,
    FocusDir,
    PickerMode,
    PaneKind,
    RundeckLevel,
    RundeckView,
    Session,
    SessionKind,
    SplitDir,
    Window,
    WindowRole,
} from "./types";

export { agentSupportsSkipPermissions } from "./commands/agentLogic";
export { normalisePinnedProjects, normaliseProjectRoots } from "./commands/settingsLogic";

const patchSession = (id: string, fn: (s: Session) => Session): void =>
    mutate((d) => {
        const cur = d.sessions[id];
        if (!cur) return;
        d.sessions[id] = fn(cur as Session);
    });

const patchWindow = (id: string, fn: (w: Window) => Window): void =>
    mutate((d) => {
        const cur = d.windows[id];
        if (!cur) return;
        d.windows[id] = fn(cur as Window);
    });

const withActiveSession = (fn: (d: StoreState, session: Session) => void): void =>
    mutate((d) => {
        const session = d.sessions[d.activeSessionId];
        if (!session) return;
        fn(d as unknown as StoreState, session as Session);
    });

const withActiveWindow = (fn: (d: StoreState, win: Window, session: Session) => void): void =>
    mutate((d) => {
        const session = d.sessions[d.activeSessionId];
        if (!session) return;
        const win = d.windows[session.activeWindowId];
        if (!win) return;
        fn(d as unknown as StoreState, win as Window, session as Session);
    });

function makeWindow(
    cwd: string,
    name: string,
    opts: {
        kind?: PaneKind;
        startup?: string;
        fixed?: boolean;
        role?: WindowRole;
    } = {},
): Window {
    const pane = makePane(cwd, opts);
    const win: Window = {
        id: newId("win"),
        name,
        role: opts.role ?? "term",
        root: pane,
        activePaneId: pane.id,
    };
    if (opts.fixed) win.fixed = true;
    return win;
}

function makeSession(kind: SessionKind, name: string, cwd: string, activeWindowId: string): Session {
    return {
        id: newId("sess"),
        name,
        kind,
        cwd,
        deploy: null,
        pinned: false,
        activeWindowId,
        activeAgentId: null,
        view: "windows",
    };
}

function projectWindows(cwd: string): Window[] {
    return [
        makeWindow(cwd, "files", { kind: "editor", fixed: true, role: "files" }),
        makeWindow(cwd, "1", { role: "term" }),
        makeWindow(cwd, "git", { kind: "git", fixed: true, role: "git" }),
        makeWindow(cwd, "search", { kind: "search", fixed: true, role: "search" }),
    ];
}

export function ensureSearchWindow(): void {
    mutate((d) => {
        for (const sid of d.sessionOrder) {
            const sess = d.sessions[sid];
            if (sess.kind !== "project") continue;
            const winIds = d.windowsBySession[sid] ?? [];
            if (winIds.some((id) => d.windows[id]?.role === "search")) continue;
            const w = makeWindow(sess.cwd, "search", {
                kind: "search",
                fixed: true,
                role: "search",
            });
            d.windows[w.id] = w;
            d.windowsBySession[sid] = [...winIds, w.id];
        }
    });
}

function attachSession(d: StoreState, session: Session, windows: Window[], agents: Agent[] = []): void {
    d.sessions[session.id] = session;
    d.sessionOrder.push(session.id);
    for (const w of windows) d.windows[w.id] = w;
    for (const a of agents) d.agents[a.id] = a;
    d.windowsBySession[session.id] = windows.map((w) => w.id);
    d.agentsBySession[session.id] = agents.map((a) => a.id);
    d.activeSessionId = session.id;
    d.zoomedPaneId = null;
    d.pickerOpen = false;
}

function dirtyPathsForWindow(st: StoreState, win: Window | undefined): string[] {
    if (!win) return [];
    return collectPanes(win.root).flatMap((p) => st.dirtyEditorPaths[p.id] ?? []);
}

function dirtyPathsForPane(st: StoreState, paneId: string): string[] {
    return st.dirtyEditorPaths[paneId] ?? [];
}

function dirtyPathsForSession(st: StoreState, sessionId: string): string[] {
    const winIds = st.windowsBySession[sessionId] ?? [];
    return winIds.flatMap((id) => dirtyPathsForWindow(st, st.windows[id]));
}

function confirmDiscardDirty(paths: string[], action: string): boolean {
    if (paths.length === 0) return true;
    const shown = paths.slice(0, 3).map(basename).join(", ");
    const more = paths.length > 3 ? ` and ${paths.length - 3} more` : "";
    const ok = window.confirm(`Discard unsaved changes in ${shown}${more}?`);
    if (!ok) notify("info", `${action} cancelled — unsaved changes remain`);
    return ok;
}

export function createProjectSession(cwd: string): void {
    mutate((d) => {
        const existing = d.sessionOrder.map((id) => d.sessions[id]).find((s) => s.cwd === cwd && s.kind === "project");
        if (existing) {
            d.pickerOpen = false;
            d.zoomedPaneId = null;
            d.activeSessionId = existing.id;
            return;
        }
        const windows = projectWindows(cwd);
        attachSession(d as unknown as StoreState, makeSession("project", basename(cwd), cwd, windows[0].id), windows);
    });
}

function cliProjectOwner(target: CliOpenTarget): Session | undefined {
    const st = getState();
    return st.sessionOrder
        .map((id) => st.sessions[id])
        .filter((session): session is Session => !!session && session.kind === "project" && isPathWithin(target.path, session.cwd))
        .sort((a, b) => b.cwd.length - a.cwd.length)[0];
}

function cliProjectRootOwner(projectRoot: string): Session | undefined {
    const st = getState();
    return st.sessionOrder
        .map((id) => st.sessions[id])
        .find((session): session is Session => !!session && session.kind === "project" && session.cwd === projectRoot);
}

/**
 * Focus the owning project for every CLI target and queue file targets for its
 * editor. Directory targets are complete as soon as their project is focused;
 * file targets are acknowledged by EditorPane only after the read succeeds.
 */
export function routeCliOpenRequest(request: CliOpenRequest): CliOpenResult[] {
    const immediate: CliOpenResult[] = [];

    for (const target of request.targets) {
        let owner = cliProjectOwner(target) ?? cliProjectRootOwner(target.projectRoot);
        if (!owner) {
            createProjectSession(target.projectRoot);
            owner = cliProjectOwner(target) ?? cliProjectRootOwner(target.projectRoot);
        }

        if (!owner) {
            immediate.push({
                requestId: request.id,
                targetId: target.id,
                paneId: null,
                path: target.path,
                error: `couldn't create a project session for ${target.projectRoot}`,
            });
            continue;
        }

        const ownerId = owner.id;
        if (target.kind === "directory") {
            mutate((d) => {
                const session = d.sessions[ownerId];
                if (!session) return;
                d.activeSessionId = ownerId;
                session.view = "windows";
                d.zoomedPaneId = null;
                d.pickerOpen = false;
                d.settingsOpen = false;
            });
            immediate.push({
                requestId: request.id,
                targetId: target.id,
                paneId: null,
                path: target.path,
                error: null,
            });
            continue;
        }

        const st = getState();
        const fileWindowId = (st.windowsBySession[ownerId] ?? []).find((id) => st.windows[id]?.role === "files");
        const fileWindow = fileWindowId ? st.windows[fileWindowId] : undefined;
        const editorPane = fileWindow ? collectPanes(fileWindow.root).find((pane) => pane.kind === "editor") : undefined;
        if (!fileWindow || !editorPane) {
            immediate.push({
                requestId: request.id,
                targetId: target.id,
                paneId: null,
                path: target.path,
                error: `project ${owner.cwd} has no files editor`,
            });
            continue;
        }

        mutate((d) => {
            const session = d.sessions[ownerId];
            const win = d.windows[fileWindow.id];
            if (!session || !win) return;
            d.activeSessionId = ownerId;
            session.activeWindowId = win.id;
            session.view = "windows";
            win.activePaneId = editorPane.id;
            d.zoomedPaneId = null;
            d.pickerOpen = false;
            d.settingsOpen = false;
            const queued = d.pendingEditorOpens[editorPane.id] ?? [];
            if (!queued.some((item) => item.requestId === request.id && item.id === target.id)) {
                queued.push({ ...target, requestId: request.id });
            }
            d.pendingEditorOpens[editorPane.id] = queued;
        });
    }

    return immediate;
}

export function consumeCliEditorOpen(paneId: string, requestId: string, targetId: string): void {
    mutate((d) => {
        const queued = d.pendingEditorOpens[paneId];
        if (!queued) return;
        const next = queued.filter((item) => item.requestId !== requestId || item.id !== targetId);
        if (next.length === 0) delete d.pendingEditorOpens[paneId];
        else d.pendingEditorOpens[paneId] = next;
    });
}

export function createCommandSession(): void {
    mutate((d) => {
        const used = new Set<number>();
        for (const id of d.sessionOrder) {
            const s = d.sessions[id];
            if (s.kind === "command") {
                const n = parseInt(s.name, 10);
                if (Number.isFinite(n)) used.add(n);
            }
        }
        let n = 1;
        while (used.has(n)) n += 1;
        const win = makeWindow("", String(n));
        attachSession(d as unknown as StoreState, makeSession("command", String(n), "", win.id), [win]);
    });
}

export function focusCommandSession(): void {
    mutate((d) => {
        const commandId = d.sessionOrder.find((id) => d.sessions[id]?.kind === "command");
        if (!commandId) return;
        d.activeSessionId = commandId;
        d.zoomedPaneId = null;
        d.pickerOpen = false;
        d.settingsOpen = false;
    });
}

export function createSshSession(alias: string): void {
    mutate((d) => {
        const existing = d.sessionOrder.map((id) => d.sessions[id]).find((s) => s.kind === "ssh" && s.name === alias);
        if (existing) {
            d.pickerOpen = false;
            d.zoomedPaneId = null;
            d.activeSessionId = existing.id;
            return;
        }
        const win = makeWindow("", alias, { startup: sshStartup(alias), role: "named" });
        attachSession(d as unknown as StoreState, makeSession("ssh", alias, "", win.id), [win]);
    });
}

function openSingletonPaneSession(kind: "aws" | "rundeck"): void {
    mutate((d) => {
        const existing = d.sessionOrder.map((id) => d.sessions[id]).find((s) => s.kind === kind);
        if (existing) {
            d.activeSessionId = existing.id;
            d.zoomedPaneId = null;
            return;
        }
        const win = makeWindow("", kind, { kind, role: kind, fixed: true });
        attachSession(d as unknown as StoreState, makeSession(kind, kind, "", win.id), [win]);
    });
}

export const openAwsSession = (): void => openSingletonPaneSession("aws");
export const openRundeckSession = (): void => openSingletonPaneSession("rundeck");

/** Prompt for a collection directory, then open it as a Bruno API workspace. */
export async function openBrunoFolder(): Promise<void> {
    try {
        const dir = await openDialog({ directory: true, multiple: false, title: "Open Bruno workspace" });
        if (typeof dir === "string") openBrunoSession(dir);
    } catch (e) {
        reportError("open bruno workspace")(e);
    }
}

/** Open (or focus) a Bruno API workspace for a collection directory. */
export function openBrunoSession(collectionPath: string): void {
    registerBrunoWorkspace(collectionPath);
    mutate((d) => {
        const existing = d.sessionOrder.map((id) => d.sessions[id]).find((s) => s.kind === "bruno" && s.bruno?.collectionPath === collectionPath);
        if (existing) {
            d.activeSessionId = existing.id;
            d.zoomedPaneId = null;
            d.pickerOpen = false;
            return;
        }
        const name = basename(collectionPath);
        const win = makeWindow(collectionPath, name, { kind: "bruno", role: "bruno", fixed: true });
        const session = makeSession("bruno", name, collectionPath, win.id);
        session.bruno = { collectionPath, selectedEnvs: {}, secretVars: {}, drafts: {} };
        attachSession(d as unknown as StoreState, session, [win]);
    });
}

function patchBrunoView(sessionId: string, patch: Partial<BrunoView>): void {
    mutate((d) => {
        const cur = d.brunoViews[sessionId] ?? DEFAULT_BRUNO_VIEW;
        d.brunoViews[sessionId] = { ...cur, ...patch };
    });
}

/** Open a request in a tab (adding it if not already open) and activate it. */
export function brunoSelectRequest(sessionId: string, path: string | null): void {
    mutate((d) => {
        const cur = d.brunoViews[sessionId] ?? DEFAULT_BRUNO_VIEW;
        if (path == null) {
            d.brunoViews[sessionId] = { ...cur, activeRequestPath: null };
            return;
        }
        const openPaths = cur.openPaths.includes(path) ? cur.openPaths : [...cur.openPaths, path];
        d.brunoViews[sessionId] = { ...cur, openPaths, activeRequestPath: path };
    });
}

/** Close an open request tab; if it was active, activate a neighbour. Unsaved drafts are kept. */
export function brunoCloseTab(sessionId: string, path: string): void {
    mutate((d) => {
        const cur = d.brunoViews[sessionId];
        if (!cur) return;
        const idx = cur.openPaths.indexOf(path);
        if (idx === -1) return;
        const openPaths = cur.openPaths.filter((p) => p !== path);
        let activeRequestPath = cur.activeRequestPath;
        if (activeRequestPath === path) activeRequestPath = openPaths[Math.min(idx, openPaths.length - 1)] ?? null;
        d.brunoViews[sessionId] = { ...cur, openPaths, activeRequestPath };
    });
}
export function brunoSetReqTab(sessionId: string, tab: BrunoReqTab): void {
    patchBrunoView(sessionId, { reqTab: tab });
}
export function brunoSetResTab(sessionId: string, tab: BrunoResTab): void {
    patchBrunoView(sessionId, { resTab: tab });
}
export function brunoSetReqPanePct(sessionId: string, reqPanePct: number): void {
    patchBrunoView(sessionId, { reqPanePct });
}
export function brunoToggleSecrets(sessionId: string, open?: boolean): void {
    mutate((d) => {
        const cur = d.brunoViews[sessionId] ?? DEFAULT_BRUNO_VIEW;
        d.brunoViews[sessionId] = { ...cur, secretsOpen: open ?? !cur.secretsOpen };
    });
}
export function brunoSelectEnv(sessionId: string, collectionPath: string, envId: string | null): void {
    mutate((d) => {
        const s = d.sessions[sessionId];
        if (s?.kind !== "bruno" || !s.bruno) return;
        if (!s.bruno.selectedEnvs) s.bruno.selectedEnvs = {};
        if (envId) s.bruno.selectedEnvs[collectionPath] = envId;
        else delete s.bruno.selectedEnvs[collectionPath];
    });
}
export function brunoSetSecret(sessionId: string, name: string, value: string): void {
    mutate((d) => {
        const s = d.sessions[sessionId];
        if (s?.kind === "bruno" && s.bruno) s.bruno.secretVars[name] = value;
    });
}
/** Stash edited (unsaved) request text by file path; pass null to clear the draft. */
export function brunoSetDraft(sessionId: string, path: string, text: string | null): void {
    mutate((d) => {
        const s = d.sessions[sessionId];
        if (s?.kind !== "bruno" || !s.bruno) return;
        if (text == null) delete s.bruno.drafts[path];
        else s.bruno.drafts[path] = text;
    });
}

/** Write the draft for a request back to its .bru file, then clear the draft. */
export async function brunoSaveRequest(sessionId: string, path: string): Promise<void> {
    const s = getState().sessions[sessionId];
    if (s?.kind !== "bruno" || !s.bruno) return;
    const draft = s.bruno.drafts[path];
    if (draft == null) return;
    const collectionPath = s.bruno.collectionPath;
    try {
        await fsapi.writeFile(path, draft);
        brunoSetDraft(sessionId, path, null);
        invalidate((kind, args) => kind === "bruno.collection" && args[0] === collectionPath);
        notify("success", `Saved ${basename(path)}`);
    } catch (e) {
        reportError("save request")(e);
    }
}

/** Save the active request of the active Bruno session (⌘S). */
export function brunoSaveActive(): void {
    const st = getState();
    const s = st.sessions[st.activeSessionId];
    if (s?.kind !== "bruno") return;
    const path = st.brunoViews[s.id]?.activeRequestPath;
    if (path) void brunoSaveRequest(s.id, path);
}

const reloadBruno = (collectionPath: string): void => invalidate((kind, args) => kind === "bruno.collection" && args[0] === collectionPath);
const safeFileName = (name: string): string => name.trim().replace(/[\\/:*?"<>|]/g, "_");

/** Create a new .bru request under a directory and select it. */
export async function brunoNewRequest(sessionId: string, dirPath: string, name: string): Promise<void> {
    const s = getState().sessions[sessionId];
    if (s?.kind !== "bruno" || !s.bruno || !name.trim()) return;
    const file = joinPath(dirPath, `${safeFileName(name)}.bru`);
    try {
        await fsapi.writeFileNew(file, serializeRequest(emptyRequest(name.trim())));
        reloadBruno(s.bruno.collectionPath);
        brunoSelectRequest(sessionId, file);
        notify("success", `Created ${name.trim()}`);
    } catch (e) {
        reportError("create request")(e);
    }
}

/** Create a new subfolder (with a folder.bru) under a directory. */
export async function brunoNewFolder(sessionId: string, parentPath: string, name: string): Promise<void> {
    const s = getState().sessions[sessionId];
    if (s?.kind !== "bruno" || !s.bruno || !name.trim()) return;
    const folder = joinPath(parentPath, safeFileName(name));
    try {
        await fsapi.createDir(folder);
        await fsapi.writeFileNew(joinPath(folder, "folder.bru"), `meta {\n  name: ${name.trim()}\n}\n`);
        reloadBruno(s.bruno.collectionPath);
        notify("success", `Created folder ${name.trim()}`);
    } catch (e) {
        reportError("create folder")(e);
    }
}

/** Rename a request: update its meta.name and move the file to match. */
export async function brunoRenameRequest(sessionId: string, path: string, name: string): Promise<void> {
    const s = getState().sessions[sessionId];
    if (s?.kind !== "bruno" || !s.bruno || !name.trim()) return;
    const newPath = joinPath(dirname(path), `${safeFileName(name)}.bru`);
    try {
        const text = s.bruno.drafts[path] ?? (await fsapi.readFile(path));
        const req = parseRequest(text);
        req.meta.name = name.trim();
        if (newPath === path) await fsapi.writeFile(path, serializeRequest(req));
        else {
            await fsapi.writeFileNew(newPath, serializeRequest(req));
            await fsapi.deletePath(path);
        }
        brunoSetDraft(sessionId, path, null);
        reloadBruno(s.bruno.collectionPath);
        // keep the tab pointing at the renamed file
        mutate((d) => {
            const v = d.brunoViews[sessionId];
            if (!v) return;
            v.openPaths = v.openPaths.map((p) => (p === path ? newPath : p));
            if (v.activeRequestPath === path) v.activeRequestPath = newPath;
        });
        notify("success", `Renamed to ${name.trim()}`);
    } catch (e) {
        reportError("rename request")(e);
    }
}

/** Delete a request file from disk. */
export async function brunoDeleteRequest(sessionId: string, path: string): Promise<void> {
    const s = getState().sessions[sessionId];
    if (s?.kind !== "bruno" || !s.bruno) return;
    try {
        await fsapi.deletePath(path);
        brunoSetDraft(sessionId, path, null);
        reloadBruno(s.bruno.collectionPath);
        brunoCloseTab(sessionId, path);
        notify("success", `Deleted ${basename(path)}`);
    } catch (e) {
        reportError("delete request")(e);
    }
}

const rundeckView = (st: StoreState, paneId: string): RundeckView => st.rundeckViews[paneId] ?? { stack: [{ kind: "matrix" }] };

export function rundeckPush(paneId: string, level: RundeckLevel): void {
    mutate((d) => {
        const cur = rundeckView(d as unknown as StoreState, paneId);
        d.rundeckViews[paneId] = { stack: [...cur.stack, level] };
    });
}

export function rundeckReplace(paneId: string, level: RundeckLevel): void {
    mutate((d) => {
        const cur = rundeckView(d as unknown as StoreState, paneId);
        const stack = cur.stack.slice(0, -1);
        stack.push(level);
        d.rundeckViews[paneId] = { stack };
    });
}

export function rundeckPop(paneId: string): void {
    mutate((d) => {
        const cur = rundeckView(d as unknown as StoreState, paneId);
        if (cur.stack.length <= 1) return;
        d.rundeckViews[paneId] = { stack: cur.stack.slice(0, -1) };
    });
}

export function rundeckPopTo(paneId: string, index: number): void {
    mutate((d) => {
        const cur = rundeckView(d as unknown as StoreState, paneId);
        const target = Math.max(0, Math.min(index, cur.stack.length - 1));
        d.rundeckViews[paneId] = { stack: cur.stack.slice(0, target + 1) };
    });
}

export function rundeckHome(paneId: string): void {
    mutate((d) => {
        d.rundeckViews[paneId] = { stack: [{ kind: "matrix" }] };
    });
}

function setRundeckProject(project: string, envFolder: string | null = null): void {
    mutate((d) => {
        d.rundeck.activeProject = project;
        d.rundeck.activeEnvFolder = envFolder;
    });
}

export function selectRundeckProject(paneId: string, project: string, envFolder: string | null = null): void {
    setRundeckProject(project, envFolder);
    rundeckHome(paneId);
}

/** Open the Rundeck session straight to a known service deploy (project + env folder). */
export function openRundeckService(target: { project: string; service: string; jobId: string; group: string | null }): void {
    openRundeckTarget(target);
}

export function openRundeckDeploy(target: { project: string; service: string; jobId: string; group: string | null; branch: string }): void {
    openRundeckTarget(target, target.branch);
}

function openRundeckTarget(target: { project: string; service: string; jobId: string; group: string | null }, branch?: string): void {
    const before = getState();
    const sourceSession = before.sessions[before.activeSessionId];
    const sourceRepoPath = sourceSession?.kind === "project" ? sourceSession.cwd : "";
    const env = inferEnv(target.project, target.group);
    const serviceLevel: RundeckLevel = {
        kind: "service",
        env,
        project: target.project,
        service: target.service,
        jobId: target.jobId,
        repoPath: sourceRepoPath,
    };
    openRundeckSession();
    const after = getState();
    const sess = Object.values(after.sessions).find((s) => s.kind === "rundeck");
    if (!sess) return;
    const win = after.windows[sess.activeWindowId];
    if (!win || win.root.type !== "pane") return;
    const paneId = win.root.id;
    setRundeckProject(target.project, envFolderOf(target.group));
    rundeckReplaceStack(paneId, [
        { kind: "matrix" },
        serviceLevel,
        ...(branch !== undefined
            ? [
                  {
                      kind: "deploy" as const,
                      env,
                      project: target.project,
                      service: target.service,
                      jobId: target.jobId,
                      branch,
                      repoPath: sourceRepoPath,
                  },
              ]
            : []),
    ]);
}

function rundeckReplaceStack(paneId: string, stack: RundeckLevel[]): void {
    mutate((d) => {
        d.rundeckViews[paneId] = { stack };
    });
}

export function selectSession(id: string): void {
    mutate((d) => {
        if (!d.sessions[id]) return;
        d.activeSessionId = id;
        d.zoomedPaneId = null;
        d.sessionSwitcher = null;
        d.pickerOpen = false;
        d.settingsOpen = false;
    });
}

export function selectLastSession(): void {
    const id = getState().lastSessionId;
    if (id) selectSession(id);
}

export function closeSession(id: string): void {
    if (!confirmDiscardDirty(dirtyPathsForSession(getState(), id), "close session")) return;
    const closingCwd = getState().sessions[id]?.cwd;
    mutate((d) => {
        if (d.sessionOrder.length <= 1) return;
        const closed = d.sessions[id];
        if (!closed) return;
        const idx = d.sessionOrder.indexOf(id);
        const winIds = d.windowsBySession[id] ?? [];
        const agentIds = d.agentsBySession[id] ?? [];
        const isSshConfig = winIds.some((windowId) => d.windows[windowId]?.role === "ssh-config");

        for (const wid of winIds) {
            const w = d.windows[wid];
            if (w) {
                for (const p of collectPanes(w.root as unknown as Window["root"])) {
                    if (d.gitModal?.ownerPaneId === p.id) d.gitModal = null;
                    delete d.editorViews[p.id];
                    delete d.pendingEditorOpens[p.id];
                    delete d.dirtyEditorPaths[p.id];
                    delete d.gitViews[p.id];
                    delete d.ecsViews[p.id];
                }
            }
            delete d.windows[wid];
        }
        for (const aid of agentIds) delete d.agents[aid];
        delete d.windowsBySession[id];
        delete d.agentsBySession[id];
        delete d.brunoViews[id];
        delete d.sessions[id];
        d.sessionOrder = d.sessionOrder.filter((x) => x !== id);

        if (d.activeSessionId === id) {
            d.activeSessionId = d.sessionOrder[Math.min(idx, d.sessionOrder.length - 1)];
        }
        if (closed.kind !== "command" && !isSshConfig) {
            d.recent = [{ kind: closed.kind, name: closed.name, cwd: closed.cwd }, ...d.recent.filter((r) => r.cwd !== closed.cwd)].slice(0, 12);
        }
        d.zoomedPaneId = null;
    });
    if (closingCwd) {
        const stillOpen = Object.values(getState().sessions).some((s) => s.cwd === closingCwd);
        if (!stillOpen) {
            void lsp.stop(closingCwd).catch(() => {});
        }
    }
}

export function closeActiveSession(): void {
    closeSession(getState().activeSessionId);
}

export function cycleSession(delta: number): void {
    mutate((d) => {
        const cur = d.sessions[d.activeSessionId];
        if (!cur) return;
        const groupIds = d.sessionOrder.filter((id) => d.sessions[id].kind === cur.kind);
        if (groupIds.length < 2) return;
        const idx = groupIds.indexOf(cur.id);
        d.activeSessionId = groupIds[(idx + delta + groupIds.length) % groupIds.length];
        d.zoomedPaneId = null;
    });
}

export function beginSessionSwitch(delta: number, releaseModifier: import("./types").KeyModifier): void {
    mutate((d) => {
        const cur = d.sessions[d.activeSessionId];
        if (!cur) return;
        const sessionIds = d.sessionOrder.filter((id) => d.sessions[id]?.kind === cur.kind);
        if (sessionIds.length < 2) return;
        const idx = sessionIds.indexOf(cur.id);
        d.sessionSwitcher = {
            sessionIds,
            selectedSessionId: sessionIds[(idx + delta + sessionIds.length) % sessionIds.length],
            releaseModifier,
        };
    });
}

export function cycleSessionSwitch(delta: number): void {
    mutate((d) => {
        const switcher = d.sessionSwitcher;
        if (!switcher) return;
        const sessionIds = switcher.sessionIds.filter((id) => d.sessions[id]);
        if (sessionIds.length < 2) {
            d.sessionSwitcher = null;
            return;
        }
        const idx = sessionIds.indexOf(switcher.selectedSessionId);
        switcher.sessionIds = sessionIds;
        switcher.selectedSessionId = sessionIds[((idx < 0 ? 0 : idx) + delta + sessionIds.length) % sessionIds.length];
    });
}

export function commitSessionSwitch(): void {
    mutate((d) => {
        const selectedId = d.sessionSwitcher?.selectedSessionId;
        if (selectedId && d.sessions[selectedId]) {
            d.activeSessionId = selectedId;
            d.zoomedPaneId = null;
        }
        d.sessionSwitcher = null;
    });
}

export function cancelSessionSwitch(): void {
    mutate((d) => {
        d.sessionSwitcher = null;
    });
}

const GROUP_ORDER: SessionKind[] = ["project", "ssh", "aws", "rundeck", "bruno", "command"];

export function cycleSessionGroup(delta: number): void {
    mutate((d) => {
        const cur = d.sessions[d.activeSessionId];
        if (!cur) return;
        const populated = GROUP_ORDER.filter((kind) => d.sessionOrder.some((id) => d.sessions[id]?.kind === kind));
        if (populated.length < 2) return;
        const curIdx = populated.indexOf(cur.kind);
        if (curIdx === -1) return;
        const nextKind = populated[(curIdx + delta + populated.length) % populated.length];
        const nextId = d.sessionOrder.find((id) => d.sessions[id]?.kind === nextKind);
        if (!nextId) return;
        d.activeSessionId = nextId;
        d.zoomedPaneId = null;
    });
}

export function setDeployTarget(target: DeployRef | null): void {
    patchSession(getState().activeSessionId, (s) => ({ ...s, deploy: target }));
}

export function splitActivePane(dir: SplitDir): void {
    withActiveWindow((d, w, session) => {
        const np = makePane(session.cwd);
        const win = d.windows[w.id];
        if (!win) return;
        win.root = splitPane(w.root, w.activePaneId, dir, np);
        win.activePaneId = np.id;
        d.zoomedPaneId = null;
    });
}

export function runCustomCommand(custom: import("../commands/registry").CustomCommand): void {
    const st = getState();
    const session = st.sessions[st.activeSessionId];
    if (!session) return;
    const startup = custom.command;
    if (custom.placement === "background") {
        void invoke<{ code: number; output: string }>("run_background_command", {
            command: custom.command,
            cwd: session.cwd || null,
            env: {
                SIKEMUX_SESSION_ID: session.id,
                SIKEMUX_SESSION_NAME: session.name,
                SIKEMUX_SESSION_KIND: session.kind,
                SIKEMUX_PROJECT: session.kind === "project" ? session.cwd : "",
            },
        })
            .then((result) => notify(result.code === 0 ? "success" : "error", `${custom.title}: ${result.output.trim() || `exit ${result.code}`}`))
            .catch(reportError(custom.title));
        return;
    }
    if (custom.placement === "popup") {
        setState({
            commandPopup: {
                id: newId("popup"),
                title: custom.title,
                startup,
                cwd: session.cwd,
                context: {
                    sessionId: session.id,
                    sessionName: session.name,
                    sessionKind: session.kind,
                    ...(session.kind === "project" && session.cwd ? { project: session.cwd } : {}),
                },
            },
        });
        return;
    }
    mutate((d) => {
        const current = d.sessions[d.activeSessionId];
        if (!current) return;
        const window = d.windows[current.activeWindowId];
        if (!window) return;
        const pane = makePane(current.cwd, { startup });
        pane.title = custom.title;
        if (custom.placement === "terminal") {
            const ids = d.windowsBySession[current.id] ?? [];
            const created = makeWindow(current.cwd, custom.title, { startup });
            d.windows[created.id] = created;
            d.windowsBySession[current.id] = [...ids, created.id];
            current.activeWindowId = created.id;
        } else if (custom.placement === "split") {
            window.root = splitPane(window.root, window.activePaneId, "row", pane);
            window.activePaneId = pane.id;
        } else {
            window.root = replacePane(window.root, window.activePaneId, pane);
            window.activePaneId = pane.id;
        }
        current.view = "windows";
        d.zoomedPaneId = null;
    });
}

export function closeCommandPopup(): void {
    setState({ commandPopup: null });
}

export function upsertCustomCommand(command: import("../commands/registry").CustomCommand): void {
    setState((s) => ({ customCommands: [...s.customCommands.filter((item) => item.id !== command.id), command] }));
}

export function deleteCustomCommand(id: string): void {
    setState((s) => ({ customCommands: s.customCommands.filter((item) => item.id !== id) }));
}

export function noteRecentCommand(key: string): void {
    setState((s) => ({ recentCommandKeys: [key, ...s.recentCommandKeys.filter((item) => item !== key)].slice(0, 20) }));
}

function stripImportedStartup(node: Window["root"]): Window["root"] {
    if (node.type === "pane") return { ...node, startup: undefined, title: node.kind === "terminal" ? "shell" : node.title };
    return { ...node, children: node.children.map(stripImportedStartup) };
}

export async function exportActiveSession(): Promise<void> {
    const state = getState();
    const session = state.sessions[state.activeSessionId];
    if (!session) return;
    const safeSession = {
        ...session,
        bruno: session.bruno ? { collectionPath: session.bruno.collectionPath, selectedEnvs: session.bruno.selectedEnvs } : undefined,
    };
    const windows = (state.windowsBySession[session.id] ?? []).map((id) => state.windows[id]).filter(Boolean);
    const agents = (state.agentsBySession[session.id] ?? [])
        .map((id) => state.agents[id])
        .filter((agent): agent is Agent => !!agent?.resumeId)
        .map(({ type, title, resumeId }) => ({ type, title, resumeId }));
    const payload = JSON.stringify({ format: "sikemux-session", version: 1, session: safeSession, windows, agents }, (key, value) =>
        key === "secretVars" || key === "drafts" || key === "startup" || key === "baselineSessionIds" ? undefined : value,
    );
    await navigator.clipboard.writeText(payload);
    notify("success", `Copied ${session.name} session bundle (secrets and startup commands stripped)`);
}

export async function importSessionFromClipboard(): Promise<void> {
    const raw = await navigator.clipboard.readText();
    // Parse and validate the complete untrusted payload before entering Immer.
    // Any error therefore leaves the store byte-for-byte unchanged.
    const bundle = parseSessionBundle(raw);
    const sourceName = bundle.session.name;
    const sourceCwd = bundle.session.cwd;
    const sourceKind = bundle.session.kind;
    mutate((d) => {
        const sessionId = newId("sess");
        const importedWindows: Window[] = [];
        for (const sourceWindow of bundle.windows) {
            const root = stripImportedStartup(cloneLayout(sourceWindow.root));
            const panes = collectPanes(root);
            const sourcePanes = collectPanes(sourceWindow.root);
            const sourceActiveIndex = sourcePanes.findIndex((pane) => pane.id === sourceWindow.activePaneId);
            importedWindows.push({
                ...sourceWindow,
                id: newId("win"),
                name: sourceWindow.name || "imported",
                root,
                activePaneId: panes[Math.max(0, sourceActiveIndex)].id,
                fixed: false,
            });
        }
        const session: Session = {
            id: sessionId,
            name: `${sourceName} imported`,
            kind: sourceKind,
            cwd: sourceCwd,
            pinned: false,
            deploy: null,
            activeWindowId: importedWindows[0].id,
            activeAgentId: null,
            view: "windows",
        };
        if (sourceKind === "bruno") session.bruno = { collectionPath: sourceCwd, selectedEnvs: {}, secretVars: {}, drafts: {} };
        attachSession(d as unknown as StoreState, session, importedWindows);
        for (const row of bundle.agents) {
            const id = newId("agent");
            d.agents[id] = {
                id,
                type: row.type,
                title: row.title,
                resumeId: row.resumeId,
                startup: agentStartup(row.type, row.resumeId),
                launchState: "dormant",
            };
            d.agentsBySession[sessionId].push(id);
        }
    });
    notify("success", "Imported session as a safe, dormant copy");
}

function closeActivePane(): void {
    withActiveWindow((d, w, session) => {
        const closingPaneId = w.activePaneId;
        if (d.gitModal?.ownerPaneId === closingPaneId) d.gitModal = null;
        const root = removePane(w.root, closingPaneId);
        if (root === null && w.fixed) return;
        d.zoomedPaneId = null;
        delete d.editorViews[closingPaneId];
        delete d.pendingEditorOpens[closingPaneId];
        delete d.dirtyEditorPaths[closingPaneId];
        delete d.gitViews[closingPaneId];
        delete d.ecsViews[closingPaneId];
        delete d.rundeckViews[closingPaneId];
        if (root === null) {
            const winIds = d.windowsBySession[session.id] ?? [];
            if (winIds.length <= 1) {
                const fresh = makeWindow(session.cwd, w.name);
                delete d.windows[w.id];
                d.windows[fresh.id] = fresh;
                d.windowsBySession[session.id] = [fresh.id];
                d.sessions[session.id].activeWindowId = fresh.id;
                return;
            }
            const idx = winIds.indexOf(w.id);
            const remaining = winIds.filter((id) => id !== w.id);
            const nextId = remaining[Math.min(idx, remaining.length - 1)];
            delete d.windows[w.id];
            d.windowsBySession[session.id] = remaining;
            d.sessions[session.id].activeWindowId = nextId;
            return;
        }
        const remaining = collectPanes(root);
        const win = d.windows[w.id];
        if (!win) return;
        win.root = root;
        win.activePaneId = remaining[0].id;
    });
}

function pruneWindowViews(d: StoreState, win: Window): void {
    for (const p of collectPanes(win.root)) {
        if (d.gitModal?.ownerPaneId === p.id) d.gitModal = null;
        delete d.editorViews[p.id];
        delete d.pendingEditorOpens[p.id];
        delete d.dirtyEditorPaths[p.id];
        delete d.gitViews[p.id];
        delete d.ecsViews[p.id];
        delete d.rundeckViews[p.id];
    }
}

function replaceWithFreshTerminalTab(d: StoreState, session: Session, closing: Window): void {
    const winIds = d.windowsBySession[session.id] ?? [];
    const fresh = makeWindow(session.cwd, closing.name, {
        fixed: closing.fixed,
        role: "term",
    });
    pruneWindowViews(d, closing);
    delete d.windows[closing.id];
    d.windows[fresh.id] = fresh;
    d.windowsBySession[session.id] = winIds.map((id) => (id === closing.id ? fresh.id : id));
    const sess = d.sessions[session.id];
    sess.activeWindowId = fresh.id;
    sess.view = "windows";
    d.zoomedPaneId = null;
}

function closeActiveTerminalTab(): void {
    withActiveSession((d, session) => {
        if (session.view !== "windows") return;
        const closing = d.windows[session.activeWindowId];
        if (!closing || closing.role !== "term") return;

        const winIds = d.windowsBySession[session.id] ?? [];
        const termIds = winIds.filter((id) => d.windows[id]?.role === "term");
        if (termIds.length <= 1 && winIds.length <= 1) {
            replaceWithFreshTerminalTab(d, session, closing);
            return;
        }

        const idx = winIds.indexOf(closing.id);
        const remaining = winIds.filter((id) => id !== closing.id);
        const isTerm = (id: string) => d.windows[id]?.role === "term";
        const before = remaining.slice(0, idx).reverse().find(isTerm);
        const after = remaining.slice(idx).find(isTerm);
        const nextId = before ?? after ?? remaining[Math.min(idx, remaining.length - 1)];

        pruneWindowViews(d, closing);
        delete d.windows[closing.id];
        d.windowsBySession[session.id] = remaining;
        const sess = d.sessions[session.id];
        sess.activeWindowId = nextId;
        sess.view = "windows";
        d.zoomedPaneId = null;
    });
}

// Agent view only ever applies to project sessions; other groups may carry a
// stale `view: "agent"` but must be treated as windowed everywhere.
const inAgentView = (s: Session): boolean => s.kind === "project" && s.view === "agent";

export function closeActiveFocusTarget(): void {
    const st = getState();
    const session = st.sessions[st.activeSessionId];
    if (!session) return;

    if (inAgentView(session)) {
        if (session.activeAgentId) closeAgent(session.activeAgentId);
        return;
    }

    const win = st.windows[session.activeWindowId];
    if (win?.role === "ssh-config") {
        closeSession(session.id);
        return;
    }

    if (session.kind === "bruno") {
        // ⌥W closes the active request tab, not the whole Bruno workspace.
        const path = st.brunoViews[session.id]?.activeRequestPath;
        if (path) brunoCloseTab(session.id, path);
        return;
    }

    if (win && collectPanes(win.root).length > 1) {
        if (!confirmDiscardDirty(dirtyPathsForPane(st, win.activePaneId), "close pane")) return;
        closeActivePane();
        return;
    }

    if (session.kind === "command") {
        closeSession(session.id);
        return;
    }

    if (win?.role === "term") {
        closeActiveTerminalTab();
        return;
    }

    closeActivePane();
}

export function focusPane(paneId: string): void {
    withActiveWindow((d, w) => {
        const win = d.windows[w.id];
        if (win) win.activePaneId = paneId;
    });
}

export function moveFocus(dir: FocusDir): void {
    withActiveWindow((d, w) => {
        const { panes } = computeLayout(w.root);
        const next = neighborPane(panes, w.activePaneId, dir);
        if (!next) return;
        const win = d.windows[w.id];
        if (win) win.activePaneId = next;
    });
}

export function resizeActivePane(dir: FocusDir): void {
    withActiveWindow((d, w) => {
        const win = d.windows[w.id];
        if (win) win.root = resizeTowards(w.root, w.activePaneId, dir);
    });
}

export function toggleZoom(): void {
    withActiveSession((d, session) => {
        if (d.zoomedPaneId) {
            d.zoomedPaneId = null;
            return;
        }
        if (session.view !== "windows") return;
        const w = d.windows[session.activeWindowId];
        if (w) d.zoomedPaneId = w.activePaneId;
    });
}

export function setSplitSizes(windowId: string, splitId: string, sizes: number[]): void {
    patchWindow(windowId, (w) => ({
        ...w,
        root: setSplitSizesFn(w.root, splitId, sizes),
    }));
}

export function newWindow(): void {
    withActiveSession((d, session) => {
        const winIds = d.windowsBySession[session.id] ?? [];
        const terminalNumbers = winIds
            .map((id) => d.windows[id])
            .filter((win) => win?.role === "term")
            .map((win) => Number.parseInt(win.name, 10))
            .filter((n) => Number.isFinite(n) && n > 0);
        const nextTerminalNumber = terminalNumbers.length === 0 ? 1 : Math.max(...terminalNumbers) + 1;
        const w = makeWindow(session.cwd, String(nextTerminalNumber));
        d.windows[w.id] = w;
        d.windowsBySession[session.id] = [...winIds, w.id];
        const sess = d.sessions[session.id];
        sess.activeWindowId = w.id;
        sess.view = "windows";
        d.zoomedPaneId = null;
    });
}

export function duplicateWindow(id: string): void {
    mutate((d) => {
        const source = d.windows[id];
        const ownerId = d.sessionOrder.find((sid) => d.windowsBySession[sid]?.includes(id));
        if (!source || !ownerId) return;
        const root = cloneLayout(source.root);
        const activePane = collectPanes(root)[0];
        const duplicate: Window = {
            ...source,
            id: newId("win"),
            name: `${source.name} copy`,
            root,
            activePaneId: activePane.id,
            fixed: false,
            role: source.role === "term" ? "term" : "named",
        };
        d.windows[duplicate.id] = duplicate;
        const ids = d.windowsBySession[ownerId] ?? [];
        const index = ids.indexOf(id);
        d.windowsBySession[ownerId] = [...ids.slice(0, index + 1), duplicate.id, ...ids.slice(index + 1)];
        d.sessions[ownerId].activeWindowId = duplicate.id;
        d.sessions[ownerId].view = "windows";
    });
}

export function closeWindowById(id: string): void {
    const st = getState();
    const closing = st.windows[id];
    if (!closing || closing.fixed) return;
    if (!confirmDiscardDirty(dirtyPathsForWindow(st, closing), "close window")) return;
    withActiveSession((d, session) => {
        const winIds = d.windowsBySession[session.id] ?? [];
        if (!winIds.includes(id) || winIds.length <= 1) return;
        const closing = d.windows[id];
        if (!closing || closing.fixed) return;
        const idx = winIds.indexOf(id);
        const remaining = winIds.filter((wid) => wid !== id);
        const sess = d.sessions[session.id];
        if (sess.activeWindowId === id) {
            let nextId = remaining[Math.min(idx, remaining.length - 1)];
            if (closing.role === "term") {
                const isTerm = (wid: string) => d.windows[wid]?.role === "term";
                const before = remaining.slice(0, idx).reverse().find(isTerm);
                const after = remaining.slice(idx).find(isTerm);
                nextId = before ?? after ?? nextId;
            }
            sess.activeWindowId = nextId;
        }
        pruneWindowViews(d, closing);
        delete d.windows[id];
        d.windowsBySession[session.id] = remaining;
        d.zoomedPaneId = null;
    });
}

export function closeActiveWindow(): void {
    const session = getState().sessions[getState().activeSessionId];
    if (session) closeWindowById(session.activeWindowId);
}

export function selectWindowId(id: string): void {
    withActiveSession((d, session) => {
        const winIds = d.windowsBySession[session.id] ?? [];
        if (!winIds.includes(id)) return;
        const sess = d.sessions[session.id];
        if (sess.activeWindowId === id && sess.view === "windows" && d.zoomedPaneId === null) {
            return;
        }
        sess.activeWindowId = id;
        sess.view = "windows";
        d.zoomedPaneId = null;
    });
}

export function selectWindowByIndex(index: number): void {
    const st = getState();
    const session = st.sessions[st.activeSessionId];
    const id = (st.windowsBySession[session?.id ?? ""] ?? [])[index];
    if (id) selectWindowId(id);
}

export function selectWindowByName(name: string): void {
    const st = getState();
    const session = st.sessions[st.activeSessionId];
    if (!session) return;
    const ids = st.windowsBySession[session.id] ?? [];
    const id = ids.find((wid) => st.windows[wid]?.name === name);
    if (id) selectWindowId(id);
}

export function selectWindowByRole(role: WindowRole): void {
    const st = getState();
    const session = st.sessions[st.activeSessionId];
    if (!session) return;
    const ids = st.windowsBySession[session.id] ?? [];
    const id = ids.find((wid) => st.windows[wid]?.role === role);
    if (id) {
        selectWindowId(id);
    } else if (role === "term" && session.kind === "project") {
        newWindow();
    }
}

const PROJECT_SLOT_ORDER: (WindowRole | "agents")[] = ["files", "term", "git", "agents", "search"];

export function selectWindowRelative(delta: number): void {
    const st = getState();
    const session = st.sessions[st.activeSessionId];
    if (!session) return;
    const winIds = st.windowsBySession[session.id] ?? [];
    const agentIds = st.agentsBySession[session.id] ?? [];

    if (session.kind !== "project") {
        if (winIds.length < 2) return;
        const idx = winIds.indexOf(session.activeWindowId);
        const next = winIds[(idx + delta + winIds.length) % winIds.length];
        selectWindowId(next);
        return;
    }

    const activeWin = st.windows[session.activeWindowId];
    const winForRole = (role: WindowRole): string | undefined => {
        if (activeWin?.role === role) return activeWin.id;
        return winIds.find((id) => st.windows[id]?.role === role);
    };

    type Slot = { kind: "win"; role: WindowRole; id: string } | { kind: "agents" };
    const slots: Slot[] = [];
    for (const slot of PROJECT_SLOT_ORDER) {
        if (slot === "agents") {
            if (agentIds.length > 0) slots.push({ kind: "agents" });
        } else {
            const id = winForRole(slot);
            if (id) slots.push({ kind: "win", role: slot, id });
        }
    }
    if (slots.length < 2) return;

    let idx: number;
    if (session.view === "agent") {
        idx = slots.findIndex((s) => s.kind === "agents");
    } else {
        const currentRole = activeWin?.role;
        idx = slots.findIndex((s) => s.kind === "win" && s.role === currentRole);
    }
    if (idx < 0) idx = 0;

    const next = slots[(idx + delta + slots.length) % slots.length];
    if (next.kind === "agents") {
        focusAgents();
    } else {
        selectWindowId(next.id);
    }
}

export function cycleAgent(delta: number): void {
    mutate((d) => {
        const session = d.sessions[d.activeSessionId];
        if (!session) return;
        const ids = d.agentsBySession[session.id] ?? [];
        if (ids.length < 2) return;
        const idx = session.activeAgentId ? ids.indexOf(session.activeAgentId) : -1;
        const base = idx < 0 ? 0 : idx;
        const sess = d.sessions[session.id];
        sess.activeAgentId = ids[(base + delta + ids.length) % ids.length];
        sess.view = "agent";
        d.zoomedPaneId = null;
    });
}

/** ⌥./⌥, — cycle whichever tab strip is currently on screen: agent tabs, terminal
 *  tabs, or the focused editor pane's open file tabs. */
export function cycleTabs(delta: number): void {
    const st = getState();
    const session = st.sessions[st.activeSessionId];
    if (!session) return;

    if (inAgentView(session)) {
        cycleAgent(delta);
        return;
    }

    if (session.kind === "bruno") {
        const open = st.brunoViews[session.id]?.openPaths ?? [];
        if (open.length < 2) return;
        const active = st.brunoViews[session.id]?.activeRequestPath;
        const idx = active ? open.indexOf(active) : -1;
        const base = idx < 0 ? 0 : idx;
        brunoSelectRequest(session.id, open[(base + delta + open.length) % open.length]);
        return;
    }

    const win = st.windows[session.activeWindowId];
    if (!win) return;

    if (win.role === "term") {
        const termIds = (st.windowsBySession[session.id] ?? []).filter((id) => st.windows[id]?.role === "term");
        if (termIds.length < 2) return;
        const idx = termIds.indexOf(win.id);
        selectWindowId(termIds[(idx + delta + termIds.length) % termIds.length]);
        return;
    }

    const pane = collectPanes(win.root).find((p) => p.id === win.activePaneId);
    if (pane?.kind !== "editor") return;
    const tabs = st.editorViews[pane.id]?.openTabs ?? [];
    if (tabs.length < 2) return;
    const active = st.editorViews[pane.id]?.activePath;
    const idx = active ? tabs.indexOf(active) : -1;
    const base = idx < 0 ? 0 : idx;
    setEditorView(pane.id, { activePath: tabs[(base + delta + tabs.length) % tabs.length] });
}

const AGENT_RESUME_CMD: Partial<Record<AgentType, (id: string) => string>> = {
    claude: (id) => `claude --resume ${id}`,
    codex: (id) => `codex resume ${id}`,
    hermes: (id) => `hermes --resume ${id}`,
    pi: (id) => `pi --session ${id}`,
    opencode: (id) => `opencode --session ${id}`,
};

const FALLBACK_AGENT_TITLE_MAX = 13;

function shellQuote(value: string): string {
    if (/^[A-Za-z0-9_./:=@+-]+$/.test(value)) return value;
    if (IS_WINDOWS) return `'${value.replace(/'/g, "''")}'`;
    return `'${value.replace(/'/g, "'\\''")}'`;
}

export function agentStartup(type: AgentType, resumeId?: string, skipPermissions = false): string {
    const cmd = resumeId ? (AGENT_RESUME_CMD[type]?.(shellQuote(resumeId)) ?? type) : type;
    const skipFlag = SKIP_PERMISSION_FLAG[type];
    return skipPermissions && skipFlag ? `${cmd} ${skipFlag}` : cmd;
}

function usableAgentSessionTitle(row: AgentSession, current: string): string {
    const title = row.title.trim();
    if (!title) return current;
    if (title.length <= FALLBACK_AGENT_TITLE_MAX && row.id.startsWith(title)) return current;
    return title;
}

export function agentSessionMetadataPending(agent: Agent): boolean {
    if (!agent.resumeId) return true;
    const title = agent.title.trim();
    if (!title || title.toLowerCase() === agent.type) return true;
    return title.length <= FALLBACK_AGENT_TITLE_MAX && agent.resumeId.startsWith(title);
}

export function toggleAgentSkipPermissions(id: string): void {
    mutate((d) => {
        const a = d.agents[id];
        if (!a) return;
        if (!agentSupportsSkipPermissions(a.type)) return;
        const next = !a.skipPermissions;
        a.skipPermissions = next;
        a.startup = agentStartup(a.type, a.resumeId, next);
    });
}

/** ⌥Y — toggle YOLO (skip-permissions) for the active agent, when one is on screen. */
export function toggleActiveAgentSkipPermissions(): void {
    const st = getState();
    const session = st.sessions[st.activeSessionId];
    if (!session || !inAgentView(session)) return;
    const id = session.activeAgentId;
    if (id) toggleAgentSkipPermissions(id);
}

export function addAgent(type: AgentType, resumeId?: string, title?: string): void {
    withActiveSession((d, session) => {
        if (session.kind !== "project") return;
        const ownedIds = d.agentsBySession[session.id] ?? [];
        const existing = resumeId ? ownedIds.map((id) => d.agents[id]).find((a) => a && a.type === type && a.resumeId === resumeId) : undefined;
        const sess = d.sessions[session.id];
        d.zoomedPaneId = null;
        if (existing) {
            sess.activeAgentId = existing.id;
            sess.view = "agent";
            return;
        }
        const agent: Agent = {
            id: newId("agent"),
            type,
            title: title ?? type,
            startup: agentStartup(type, resumeId),
            resumeId,
            createdAt: Date.now(),
            launchState: "live",
        };
        // Fresh agents (no resumeId) record the sessions that already exist so
        // reconciliation never adopts the session you were just in. The rail
        // keeps this list warm; on a cold cache we fall back to an mtime check.
        if (!resumeId) {
            const known = peekResource(agentSessionsR, type, session.cwd);
            if (known) agent.baselineSessionIds = known.map((row) => row.id);
        }
        d.agents[agent.id] = agent;
        d.agentsBySession[session.id] = [...ownedIds, agent.id];
        sess.activeAgentId = agent.id;
        sess.view = "agent";
    });
}

export function reconcileAgentSessions(type: AgentType, cwd: string, rows: AgentSession[]): void {
    if (rows.length === 0) return;
    mutate((d) => {
        const rowById = new Map(rows.map((row) => [row.id, row]));
        const matchingAgents: Agent[] = [];
        for (const sessionId of d.sessionOrder) {
            const session = d.sessions[sessionId];
            if (session?.kind !== "project" || session.cwd !== cwd) continue;
            for (const agentId of d.agentsBySession[sessionId] ?? []) {
                const agent = d.agents[agentId];
                if (agent?.type === type) matchingAgents.push(agent);
            }
        }
        if (matchingAgents.length === 0) return;

        const claimed = new Set<string>();
        for (const agent of matchingAgents) {
            if (!agent.resumeId) continue;
            claimed.add(agent.resumeId);
            const row = rowById.get(agent.resumeId);
            if (!row) continue;
            const nextTitle = usableAgentSessionTitle(row, agent.title);
            if (nextTitle !== agent.title) {
                agent.title = nextTitle;
            }
        }

        const candidates = rows.filter((row) => !claimed.has(row.id)).sort((a, b) => b.mtime - a.mtime);
        if (candidates.length === 0) return;

        const freshAgents = matchingAgents.filter((agent) => !agent.resumeId).sort((a, b) => (b.createdAt ?? 0) - (a.createdAt ?? 0));
        for (const agent of freshAgents) {
            // Only adopt a session that didn't exist when this agent launched,
            // otherwise it grabs the session you were just in and renames its
            // tab. `baselineSessionIds` is the snapshot taken at creation; when
            // it's missing (legacy agent / cold cache) fall back to "written at
            // or after launch", since a genuinely new session file appears
            // post-launch — never before.
            const baseline = agent.baselineSessionIds;
            const launchedAt = Math.floor((agent.createdAt ?? Date.now()) / 1000);
            const idx = candidates.findIndex((row) => (baseline ? !baseline.includes(row.id) : row.mtime >= launchedAt));
            if (idx < 0) continue;
            const [row] = candidates.splice(idx, 1);
            agent.resumeId = row.id;
            agent.title = usableAgentSessionTitle(row, agent.title);
            agent.startup = agentStartup(agent.type, agent.resumeId, agent.skipPermissions ?? false);
            delete agent.baselineSessionIds;
            claimed.add(row.id);
        }
    });
}

export function selectAgent(id: string): void {
    withActiveSession((d, session) => {
        const sess = d.sessions[session.id];
        sess.activeAgentId = id;
        sess.view = "agent";
        const activity = d.agentActivity[id];
        if (activity) {
            activity.unread = false;
            if (activity.state === "done") activity.state = "idle";
        }
    });
}

export function resumeAgent(id: string): void {
    mutate((d) => {
        const agent = d.agents[id];
        if (agent) agent.launchState = "live";
    });
}

export function noteAgentActivity(id: string, event: "working" | "complete" | import("./agentStatus").AgentStateEvent): void {
    mutate((d) => {
        if (!d.agents[id]) return;
        const ownerId = d.sessionOrder.find((sid) => (d.agentsBySession[sid] ?? []).includes(id));
        const owner = ownerId ? d.sessions[ownerId] : undefined;
        const visible = !!owner && owner.id === d.activeSessionId && owner.view === "agent" && owner.activeAgentId === id;
        const previous = d.agentActivity[id];
        const semantic =
            typeof event === "string"
                ? {
                      agentId: id,
                      state: event === "complete" ? ("idle" as const) : ("working" as const),
                      sequence: (previous?.sequence ?? 0) + 1,
                      source: "activity" as const,
                      confidence: "low" as const,
                      reason: event === "complete" ? "legacy activity settled" : "terminal input or output",
                  }
                : event;
        const reduced = reduceAgentState(previous, semantic, visible);
        if (reduced) d.agentActivity[id] = reduced;
    });
}

export function clearAgentUnread(id: string): void {
    mutate((d) => {
        const activity = d.agentActivity[id];
        if (activity) {
            activity.unread = false;
            if (activity.state === "done") activity.state = "idle";
        }
    });
}

export function closeAgent(id: string): void {
    mutate((d) => {
        const ownerId = d.sessionOrder.find((sid) => (d.agentsBySession[sid] ?? []).includes(id));
        if (!ownerId) return;
        const owner = d.sessions[ownerId];
        const ownedIds = (d.agentsBySession[ownerId] ?? []).filter((aid) => aid !== id);
        const wasActive = owner.activeAgentId === id;
        delete d.agents[id];
        delete d.agentActivity[id];
        d.agentsBySession[ownerId] = ownedIds;
        if (wasActive) {
            owner.activeAgentId = ownedIds[0] ?? null;
            if (ownedIds.length === 0) owner.view = "windows";
        }
    });
}

export function focusAgents(): void {
    // Agents only exist in project sessions. Other groups (bruno, aws, rundeck,
    // ssh, command) have no agents and no way back out of "agent" view, so the
    // The agent pane shortcut (⌥4) is a no-op there.
    if (getState().sessions[getState().activeSessionId]?.kind !== "project") return;
    withActiveSession((d, session) => {
        const ids = d.agentsBySession[session.id] ?? [];
        const sess = d.sessions[session.id];
        sess.view = "agent";
        sess.activeAgentId = session.activeAgentId ?? ids[0] ?? null;
        d.zoomedPaneId = null;
    });
    emit({ type: "agent-focus", sessionId: getState().activeSessionId });
}

export const setHome = (home: string): void => setState({ home });
export const setLastSessionId = (id: string): void => setState({ lastSessionId: id });
export const setTerminalTitle = (paneId: string, title: string): void =>
    setState((s) => ({ terminalTitles: { ...s.terminalTitles, [paneId]: title } }));
export const openPicker = (mode: PickerMode = "all"): void => setState({ pickerOpen: true, pickerMode: mode, rundeckJobPaletteOpen: false });
export const closePicker = (): void => setState({ pickerOpen: false });
export const openAgentPalette = (): void => setState({ agentPaletteOpen: true, rundeckJobPaletteOpen: false });
export const closeAgentPalette = (): void => setState({ agentPaletteOpen: false });
export const openCommandPalette = (): void => setState({ commandPaletteOpen: true });
export const closeCommandPalette = (): void => setState({ commandPaletteOpen: false });
export const toggleCommandPalette = (): void => setState((s) => ({ commandPaletteOpen: !s.commandPaletteOpen }));
export const openOnboarding = (): void => setState({ onboardingOpen: true });
export const closeOnboarding = (complete = true): void => setState({ onboardingOpen: false, ...(complete ? { onboardingComplete: true } : {}) });
export const openDiagnostics = (): void => setState({ diagnosticsOpen: true });
export const closeDiagnostics = (): void => setState({ diagnosticsOpen: false });
export const openWhatsNew = (): void => setState({ whatsNewOpen: true });
export const closeWhatsNew = (): void =>
    setState((s) => ({ whatsNewOpen: false, lastSeenVersion: s.lastReleaseNotes?.version ?? s.lastSeenVersion }));
export const openFilePalette = (): void => setState({ filePaletteOpen: true, rundeckJobPaletteOpen: false });
export const closeFilePalette = (): void => setState({ filePaletteOpen: false });
export const openRundeckJobPalette = (): void =>
    setState({ rundeckJobPaletteOpen: true, pickerOpen: false, filePaletteOpen: false, agentPaletteOpen: false });
export const closeRundeckJobPalette = (): void => setState({ rundeckJobPaletteOpen: false });
export const openBrunoReqPalette = (): void =>
    setState({ brunoReqPaletteOpen: true, brunoEnvPaletteOpen: false, filePaletteOpen: false, agentPaletteOpen: false, pickerOpen: false });
export const closeBrunoReqPalette = (): void => setState({ brunoReqPaletteOpen: false });
export const openBrunoEnvPalette = (): void =>
    setState({ brunoEnvPaletteOpen: true, brunoReqPaletteOpen: false, filePaletteOpen: false, agentPaletteOpen: false, pickerOpen: false });
export const closeBrunoEnvPalette = (): void => setState({ brunoEnvPaletteOpen: false });
export const openSettings = (): void => setState({ settingsOpen: true });
export const closeSettings = (): void => setState({ settingsOpen: false });
export const toggleSettings = (): void => setState((s) => ({ settingsOpen: !s.settingsOpen }));
export async function openSshConfigEditor(): Promise<void> {
    let configPath: string;
    try {
        configPath = await sshApi.configEnsure();
    } catch (error) {
        reportError("open SSH config")(error);
        return;
    }
    const sshDir = dirname(configPath);

    mutate((d) => {
        let owner = d.sessionOrder.find((sessionId) =>
            (d.windowsBySession[sessionId] ?? []).some((windowId) => d.windows[windowId]?.role === "ssh-config"),
        );
        const targetId = owner ? (d.windowsBySession[owner] ?? []).find((windowId) => d.windows[windowId]?.role === "ssh-config") : undefined;
        let target = targetId ? d.windows[targetId] : undefined;
        let editorPane = target ? collectPanes(target.root).find((pane) => pane.kind === "editor") : undefined;

        // Replace the short-lived bespoke SSH pane shape from development builds.
        if (!target || !editorPane) {
            const stale = target;
            target = makeWindow(sshDir, "ssh config", { kind: "editor", role: "ssh-config" });
            editorPane = target.root.type === "pane" ? target.root : undefined;
            d.windows[target.id] = target;
            if (stale && owner) {
                pruneWindowViews(d, stale);
                delete d.windows[stale.id];
                d.windowsBySession[owner] = (d.windowsBySession[owner] ?? []).map((id) => (id === stale.id ? target!.id : id));
            }
        }
        if (!editorPane) return;

        // Older builds attached this window to whichever project happened to be
        // active. Detach it and give it its own SSH-side session instead.
        if (owner && d.sessions[owner]?.kind !== "ssh") {
            const formerIds = d.windowsBySession[owner] ?? [];
            const remaining = formerIds.filter((id) => id !== target!.id);
            d.windowsBySession[owner] = remaining;
            if (d.sessions[owner].activeWindowId === target.id && remaining.length > 0) {
                d.sessions[owner].activeWindowId = remaining[0];
            }
            owner = undefined;
        }

        let configSession = owner ? d.sessions[owner] : undefined;
        if (!configSession) {
            configSession = makeSession("ssh", "SSH config", sshDir, target.id);
            attachSession(d as unknown as StoreState, configSession, [target]);
            owner = configSession.id;
        } else {
            d.activeSessionId = configSession.id;
            d.zoomedPaneId = null;
            d.pickerOpen = false;
        }

        const editorView = d.editorViews[editorPane.id] ?? { openTabs: [], activePath: null, treeWidth: 210 };
        if (!editorView.openTabs.includes(configPath)) editorView.openTabs.push(configPath);
        editorView.activePath = configPath;
        d.editorViews[editorPane.id] = editorView;

        configSession.activeWindowId = target.id;
        target.activePaneId = editorPane.id;
        configSession.view = "windows";
        d.zoomedPaneId = null;
        d.settingsOpen = false;
    });
}
export const toggleLeftRail = (): void => setState((s) => ({ leftRailOpen: !s.leftRailOpen }));
export const toggleRightRail = (): void => setState((s) => ({ rightRailOpen: !s.rightRailOpen }));
export const toggleZen = (): void => setState((s) => ({ zenMode: !s.zenMode }));

function focusSessionWindowRole(role: WindowRole): void {
    withActiveSession((d, session) => {
        const target = (d.windowsBySession[session.id] ?? []).find((id) => d.windows[id]?.role === role);
        if (!target) return;
        if (session.activeWindowId === target && session.view === "windows" && d.zoomedPaneId === null) return;
        d.zoomedPaneId = null;
        const sess = d.sessions[session.id];
        sess.activeWindowId = target;
        sess.view = "windows";
    });
}

export function requestOpenFile(path: string, line?: number, character?: number): void {
    focusSessionWindowRole("files");
    emit({ type: "open-file", path, line, character });
}

export function openGitPane(): void {
    focusSessionWindowRole("git");
}

export function setThemeId(id: string): void {
    applyTheme(id);
    setState({ themeId: id, themeMode: "manual" });
}

export function applySystemTheme(dark: boolean): void {
    const state = getState();
    if (state.themeMode !== "system") return;
    const id = dark ? state.systemDarkThemeId : state.systemLightThemeId;
    applyTheme(id);
    setState({ themeId: id });
}

export function setThemeMode(mode: "manual" | "system"): void {
    setState({ themeMode: mode });
    if (mode === "system") applySystemTheme(window.matchMedia("(prefers-color-scheme: dark)").matches);
}

function setSystemThemeId(mode: "light" | "dark", id: string): void {
    const state = getState();
    const exists = !!THEMES_BY_ID[id] || state.customThemes.some((theme) => theme.id === id);
    if (!exists) return;
    setState(mode === "light" ? { systemLightThemeId: id } : { systemDarkThemeId: id });
    if (state.themeMode !== "system") return;
    const hostIsDark = window.matchMedia("(prefers-color-scheme: dark)").matches;
    if (hostIsDark === (mode === "dark")) {
        applyTheme(id);
        setState({ themeId: id });
    }
}

export const setSystemLightThemeId = (id: string): void => setSystemThemeId("light", id);
export const setSystemDarkThemeId = (id: string): void => setSystemThemeId("dark", id);

/** Live-apply a draft theme to the whole UI without persisting it — drives the theme editor preview. */
export function previewThemeDraft(theme: Theme): void {
    previewTheme(theme);
}

/** Discard any active preview and re-apply the persisted theme selection. */
export function cancelThemePreview(): void {
    applyTheme(getState().themeId);
}

/** Insert or overwrite a custom theme (matched by id), register it, and make it the active theme. */
export function saveCustomTheme(theme: Theme): void {
    setState((s) => {
        const idx = s.customThemes.findIndex((t) => t.id === theme.id);
        const customThemes = idx >= 0 ? s.customThemes.map((t, i) => (i === idx ? theme : t)) : [...s.customThemes, theme];
        return { customThemes };
    });
    registerCustomThemes(getState().customThemes);
    setThemeId(theme.id);
}

export function deleteCustomTheme(id: string): void {
    setState((s) => ({ customThemes: s.customThemes.filter((t) => t.id !== id) }));
    registerCustomThemes(getState().customThemes);
    if (getState().themeId === id) setThemeId(DEFAULT_THEME_ID);
}

export function duplicateCustomTheme(id: string): void {
    const src = getState().customThemes.find((t) => t.id === id);
    if (!src) return;
    saveCustomTheme(cloneTheme(src, { id: `custom-${Date.now().toString(36)}`, name: `${src.name} copy` }));
}

export function setWindowOpacity(v: number): void {
    const value = Number.isFinite(v) ? v : 1;
    applyWindowOpacity(value);
    setState({ windowOpacity: value });
}

export function setWindowBlur(v: number): void {
    const value = Number.isFinite(v) ? Math.round(v) : 0;
    void invoke("set_window_blur", { radius: value }).catch(swallow("set_window_blur"));
    setState({ windowBlur: value });
}

export const setCloudBrowser = (v: string): void => setState({ cloudBrowser: v.trim() });
export const setCloudBrowserShortcut = (v: string): void => setState({ cloudBrowserShortcut: v.trim() });
export const setRestoreAgentTabs = (value: boolean): void => setState({ restoreAgentTabs: value, ...(!value ? { autoResumeAgents: false } : {}) });
export const setAutoResumeAgents = (value: boolean): void =>
    setState({ autoResumeAgents: value, restoreAgentTabs: value ? true : getState().restoreAgentTabs });
export const setRailDensity = (value: import("./types").RailDensity): void => setState({ railDensity: value });
export const setUpdateChannel = (value: "stable" | "preview"): void => setState({ updateChannel: value, pendingUpdate: null });
export const patchNotificationPreferences = (patch: Partial<import("./types").NotificationPreferences>): void =>
    setState((s) => ({ notificationPreferences: { ...s.notificationPreferences, ...patch } }));

export function setKeybinding(id: import("../keybindings").KeybindingActionId, binding: string | null): void {
    setState((s) => ({ keybindingOverrides: { ...s.keybindingOverrides, [id]: binding } }));
}

export function resetKeybinding(id: import("../keybindings").KeybindingActionId): void {
    setState((s) => {
        const keybindingOverrides = { ...s.keybindingOverrides };
        delete keybindingOverrides[id];
        return { keybindingOverrides };
    });
}

export const resetAllKeybindings = (): void => setState({ keybindingOverrides: {} });

export function addProjectRoot(path: string, depth = 1): void {
    setState((s) => (s.projectRoots.some((r) => r.path === path) ? {} : { projectRoots: [...s.projectRoots, { path, depth }] }));
    invalidate((kind) => kind === projectRootsScanR.kind);
}

export function addPinnedProject(path: string): void {
    setState((s) => (s.pinnedProjects.some((p) => p.path === path) ? {} : { pinnedProjects: [...s.pinnedProjects, { path }] }));
    invalidate((kind) => kind === projectRootsScanR.kind);
}

/** Remember a Bruno workspace so it stays reopenable after its session is closed. Most-recent-first. */
export function registerBrunoWorkspace(path: string): void {
    setState((s) => ({ brunoWorkspaces: [path, ...s.brunoWorkspaces.filter((p) => p !== path)] }));
}

/** Forget an imported Bruno workspace entirely (removes it from the picker). */
export function removeBrunoWorkspace(path: string): void {
    setState((s) => ({ brunoWorkspaces: s.brunoWorkspaces.filter((p) => p !== path) }));
}

export function removePinnedProject(path: string): void {
    setState((s) => ({
        pinnedProjects: s.pinnedProjects.filter((p) => p.path !== path),
    }));
    invalidate((kind) => kind === projectRootsScanR.kind);
}

export function removeProjectRoot(path: string): void {
    setState((s) => ({
        projectRoots: s.projectRoots.filter((r) => r.path !== path),
    }));
    invalidate((kind) => kind === projectRootsScanR.kind);
}

export function setProjectRootDepth(path: string, depth: number): void {
    const d = Math.max(0, Math.round(Number.isFinite(depth) ? depth : 1));
    setState((s) => ({
        projectRoots: s.projectRoots.map((r) => (r.path === path ? { ...r, depth: d } : r)),
    }));
    invalidate((kind) => kind === projectRootsScanR.kind);
}

export const setAwsProfile = (name: string | null): void => setState({ awsProfile: name });
export const setAwsService = (s: AwsService): void => setState({ awsService: s });
export const openAwsAuthModal = (profile: string, ssoStartUrl: string | null): void => setState({ awsAuthModal: { profile, ssoStartUrl } });
export const closeAwsAuthModal = (): void => setState({ awsAuthModal: null });

export async function runAwsSsoLogin(profile: string): Promise<boolean> {
    const result = await awsApi.ssoLogin(profile);
    if (result.success) {
        invalidate((kind, args) => kind === awsIdentityR.kind && args[0] === profile);
        await fetchResource(awsIdentityR, profile, true).catch(swallow("awsIdentityR refetch"));
    }
    return result.success;
}

export function openEditorTab(paneId: string, path: string, activate = true): void {
    mutate((d) => {
        const cur = d.editorViews[paneId] ?? { openTabs: [], activePath: null, treeWidth: 210 };
        if (!cur.openTabs.includes(path)) cur.openTabs.push(path);
        if (activate) cur.activePath = path;
        d.editorViews[paneId] = cur;
    });
}

export function setEditorView(paneId: string, patch: Partial<StoreState["editorViews"][string]>): void {
    mutate((d) => {
        const cur = d.editorViews[paneId] ?? {
            openTabs: [],
            activePath: null,
            treeWidth: 210,
        };
        d.editorViews[paneId] = { ...cur, ...patch };
    });
}

export function setEditorDirtyPaths(paneId: string, paths: string[]): void {
    mutate((d) => {
        if (paths.length === 0) delete d.dirtyEditorPaths[paneId];
        else d.dirtyEditorPaths[paneId] = paths;
    });
}

export function setGitView(paneId: string, patch: Partial<StoreState["gitViews"][string]>): void {
    mutate((d) => {
        const cur = (d.gitViews[paneId] ?? DEFAULT_GIT_VIEW) as StoreState["gitViews"][string];
        d.gitViews[paneId] = { ...cur, ...patch };
    });
}

export function setEcsLevel(paneId: string, level: EcsLevel): void {
    mutate((d) => {
        d.ecsViews[paneId] = level;
    });
}

export function setBillingExpandedMonth(profile: string, month: string | null): void {
    mutate((d) => {
        d.expandedBillingMonth[profile] = month;
    });
}

function searchViewFor(sessionId: string) {
    const st = getState();
    return st.globalSearchBySession[sessionId] ?? DEFAULT_GLOBAL_SEARCH_VIEW;
}

export function focusGlobalSearch(seed?: string): void {
    const st = getState();
    const session = st.sessions[st.activeSessionId];
    if (!session || session.kind !== "project") return;
    if (seed && seed.trim().length > 0) {
        const oneLine = seed.split(/\r?\n/).find((l) => l.trim().length > 0) ?? seed.trim();
        setGlobalSearchQuery(session.id, oneLine);
    }
    const ids = st.windowsBySession[session.id] ?? [];
    const target = ids.find((id) => st.windows[id]?.role === "search");
    if (target) selectWindowId(target);
    emit({ type: "search-focus", sessionId: session.id });
}

export function setGlobalSearchQuery(sessionId: string, query: string): void {
    const cur = searchViewFor(sessionId);
    mutate((d) => {
        d.globalSearchBySession[sessionId] = { ...cur, query };
    });
}

export function setGlobalSearchOption<K extends keyof typeof DEFAULT_GLOBAL_SEARCH_VIEW.options>(
    sessionId: string,
    key: K,
    value: (typeof DEFAULT_GLOBAL_SEARCH_VIEW.options)[K],
): void {
    const cur = searchViewFor(sessionId);
    mutate((d) => {
        d.globalSearchBySession[sessionId] = {
            ...cur,
            options: { ...cur.options, [key]: value },
        };
    });
}

export function toggleGlobalSearchFileCollapsed(sessionId: string, path: string): void {
    const cur = searchViewFor(sessionId);
    const wasCollapsed = !!cur.collapsed[path];
    const next = { ...cur.collapsed };
    if (wasCollapsed) delete next[path];
    else next[path] = true;
    mutate((d) => {
        d.globalSearchBySession[sessionId] = { ...cur, collapsed: next };
    });
}

export function setGlobalSearchReplace(sessionId: string, replace: string): void {
    const cur = searchViewFor(sessionId);
    mutate((d) => {
        d.globalSearchBySession[sessionId] = { ...cur, replace };
    });
}

export function setGlobalSearchSelected(sessionId: string, selected: { path: string; matchIndex: number } | null): void {
    const cur = searchViewFor(sessionId);
    mutate((d) => {
        d.globalSearchBySession[sessionId] = { ...cur, selected };
    });
}

export function toggleGlobalSearchReplaceOpen(sessionId: string): void {
    const cur = searchViewFor(sessionId);
    mutate((d) => {
        d.globalSearchBySession[sessionId] = { ...cur, replaceOpen: !cur.replaceOpen };
    });
}
