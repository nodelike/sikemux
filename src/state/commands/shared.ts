import { pluginDocuments } from "../../plugins/documents";
import { browserApi } from "../../api/browser";
import { basename } from "../../lib/paths";
import { taskPtyBindings } from "../../tasks/nativeRuntime";
import { emit } from "../bus";
import { getState, mutate, setState, type StoreState } from "../store";
import { deskEditorId } from "../desks";
import { notify, reportError } from "../toast";
import { confirmDialog } from "../dialog";
import { editorPaneOf } from "../selectors";
import { collectPanes, makePane, newId } from "../layout";
import type { PaneKind, Session, SessionKind, Window, WindowRole } from "../types";
import { focusPane } from "./panes";
import { selectWindowId } from "./tabs";

export const patchWindow = (id: string, fn: (w: Window) => Window): void =>
    mutate((d) => {
        const cur = d.windows[id];
        if (!cur) return;
        d.windows[id] = fn(cur as Window);
    });

export const withActiveSession = (fn: (d: StoreState, session: Session) => void): void =>
    mutate((d) => {
        const session = d.sessions[d.activeSessionId];
        if (!session) return;
        fn(d as unknown as StoreState, session as Session);
    });

export const withActiveWindow = (fn: (d: StoreState, win: Window, session: Session) => void): void =>
    mutate((d) => {
        const session = d.sessions[d.activeSessionId];
        if (!session) return;
        const win = d.windows[session.activeWindowId];
        if (!win) return;
        fn(d as unknown as StoreState, win as Window, session as Session);
    });

export function makeWindow(
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

export function makeSession(kind: SessionKind, name: string, cwd: string, activeWindowId: string): Session {
    return {
        id: newId("sess"),
        name,
        kind,
        cwd,
        pinned: false,
        activeWindowId,
    };
}

/** Tool panes that can be split into another tab and still be found there by their rail button and shortcut. */
const SPLITTABLE_TOOLS: ReadonlySet<PaneKind> = new Set(["git", "search"]);

/**
 * Focus the session's window for `role`, creating a closable one if it has
 * none. `seedEditorPath` pre-opens a file so a freshly created editor hydrates
 * from the store the same way a restored session does.
 */
export function ensureRoleWindow(role: WindowRole, kind: PaneKind, name: string, seedEditorPath?: string): void {
    const st = getState();
    const session = st.sessions[st.activeSessionId];
    if (!session || session.kind !== "project") return;
    const ids = st.windowsBySession[session.id] ?? [];
    const existing = ids.find((id) => st.windows[id]?.role === role);
    if (existing) {
        selectWindowId(existing);
        return;
    }
    // Git or search split into another tab is still the one to go to.
    const holder = SPLITTABLE_TOOLS.has(kind)
        ? ids.find((id) => st.windows[id] && collectPanes(st.windows[id].root).some((pane) => pane.kind === kind))
        : undefined;
    if (holder) {
        const pane = collectPanes(st.windows[holder].root).find((candidate) => candidate.kind === kind)!;
        selectWindowId(holder);
        focusPane(pane.id);
        return;
    }
    mutate((d) => {
        const sess = d.sessions[session.id];
        const w = makeWindow(sess.cwd, name, { kind, role });
        d.windows[w.id] = w;
        d.windowsBySession[session.id] = [...(d.windowsBySession[session.id] ?? []), w.id];
        if (seedEditorPath) d.editorViews[w.activePaneId] = { openTabs: [seedEditorPath], activePath: seedEditorPath };
        sess.activeWindowId = w.id;
        d.zoomedPaneId = null;
    });
}

export function attachSession(d: StoreState, session: Session, windows: Window[]): void {
    d.sessions[session.id] = session;
    d.sessionOrder.push(session.id);
    for (const w of windows) d.windows[w.id] = w;
    d.windowsBySession[session.id] = windows.map((w) => w.id);
    d.activeSessionId = session.id;
    d.zoomedPaneId = null;
    d.pickerOpen = false;
}

/** A project's session, opened behind the one in front if the project is not open yet. Returns its id. */
export function projectSessionInBackground(d: StoreState, cwd: string): string {
    const existing = d.sessionOrder.map((id) => d.sessions[id]).find((s) => s.cwd === cwd && s.kind === "project");
    if (existing) return existing.id;
    const window = makeWindow(cwd, "Terminal", { role: "term" });
    const session = makeSession("project", basename(cwd), cwd, window.id);
    d.sessions[session.id] = session;
    d.sessionOrder.push(session.id);
    d.windows[window.id] = window;
    d.windowsBySession[session.id] = [window.id];
    return session.id;
}

/** Brings a project's session forward, opening one with a terminal if the project is not open yet. Returns its id. */
/**
 * Opening a project while a space is shown keeps it in view: a project in no
 * space joins the shown space, and one in another space brings that space up.
 */
export function keepOpenedProjectInView(cwd: string): void {
    const { activeSpaceId, projectSpaces } = getState();
    if (activeSpaceId === null || projectSpaces[cwd] === activeSpaceId) return;
    const placed = projectSpaces[cwd];
    if (placed) setState({ activeSpaceId: placed });
    else setState({ projectSpaces: { ...projectSpaces, [cwd]: activeSpaceId } });
}

export function openProjectSession(d: StoreState, cwd: string): string {
    const existing = d.sessionOrder.map((id) => d.sessions[id]).find((s) => s.cwd === cwd && s.kind === "project");
    if (!existing) {
        const windows = [makeWindow(cwd, "Terminal", { role: "term" })];
        attachSession(d, makeSession("project", basename(cwd), cwd, windows[0].id), windows);
        return d.activeSessionId;
    }
    d.pickerOpen = false;
    d.zoomedPaneId = null;
    d.activeSessionId = existing.id;
    return existing.id;
}

export function dirtyPathsForWindow(st: StoreState, win: Window | undefined): string[] {
    if (!win) return [];
    return collectPanes(win.root).flatMap((p) => dirtyPathsForPane(st, p.id));
}

/* A desk's editor keeps its edits under its agent rather than its pane. */
export function dirtyPathsForPane(st: StoreState, paneId: string): string[] {
    const deskAgentId = st.deskPanes[paneId];
    return [...(st.dirtyEditorPaths[paneId] ?? []), ...(deskAgentId ? (st.dirtyEditorPaths[deskEditorId(deskAgentId)] ?? []) : [])];
}

export function dirtyPathsForSession(st: StoreState, sessionId: string): string[] {
    const winIds = st.windowsBySession[sessionId] ?? [];
    return winIds.flatMap((id) => dirtyPathsForWindow(st, st.windows[id]));
}

/**
 * Runs `proceed` once the user accepts losing the listed edits. Stays
 * synchronous when nothing is dirty so the common close path has no extra tick.
 */
export function guardDiscardDirty(paths: string[], action: string, proceed: () => void): void {
    if (paths.length === 0) {
        proceed();
        return;
    }
    const shown = paths.slice(0, 3).map(basename).join(", ");
    const more = paths.length > 3 ? ` and ${paths.length - 3} more` : "";
    void confirmDialog({
        title: "Discard unsaved changes?",
        body: `Edits in ${shown}${more} will be lost.`,
        confirmLabel: "Discard",
        destructive: true,
    }).then((ok) => {
        if (ok) proceed();
        else notify("info", `${action} cancelled — unsaved changes remain`);
    });
}

export function busyAgentIds(state: Pick<StoreState, "agentActivity">, agentIds: readonly string[]): string[] {
    return agentIds.filter((id) => {
        const backend = state.agentActivity[id]?.backendState;
        return backend === "working" || backend === "blocked";
    });
}

/** Closing whatever holds these agents stops them, so it asks first. */
export function guardStopAgents(agentIds: readonly string[], title: string, proceed: () => void): void {
    if (agentIds.length === 0) {
        proceed();
        return;
    }
    const agents = getState().agents;
    const shown = agentIds.slice(0, 3).map((id) => agents[id]?.title ?? "agent");
    const more = agentIds.length > 3 ? ` and ${agentIds.length - 3} more` : "";
    void confirmDialog({
        title,
        body: `${shown.join(", ")}${more} ${agentIds.length === 1 ? "stops" : "stop"} with it.`,
        confirmLabel: "Close",
        destructive: true,
    }).then((ok) => {
        if (ok) proceed();
    });
}

/* Nothing shows this agent's browser once its pane is gone, so the strip stops
   being worth reading — and so do the pages a restored pane never opened. */
export function dropDeskPaneState(d: StoreState, paneId: string): void {
    const agentId = d.deskPanes[paneId];
    delete d.deskPanes[paneId];
    delete d.deskRestores[paneId];
    if (agentId && !Object.values(d.deskPanes).includes(agentId)) delete d.browserStrips[agentId];
}

/** An agent's desk goes with the agent: its pages, its files and its terminals. */
export function closeAgentDesk(agentId: string): void {
    for (const terminal of getState().desks[agentId]?.terminals ?? []) taskPtyBindings.release(terminal.id);
    mutate((d) => {
        delete d.desks[agentId];
        delete d.editorViews[deskEditorId(agentId)];
        delete d.dirtyEditorPaths[deskEditorId(agentId)];
    });
    void browserApi.closeAgent(agentId).catch(reportError("close agent browser"));
}

export function disposePaneState(d: StoreState, paneId: string): void {
    emit({ type: "pane-closed", paneId });
    if (d.gitModal?.ownerPaneId === paneId) d.gitModal = null;
    delete d.editorViews[paneId];
    delete d.pendingEditorOpens[paneId];
    delete d.dirtyEditorPaths[paneId];
    delete d.gitViews[paneId];
    dropDeskPaneState(d, paneId);
    delete d.terminalTitles[paneId];
    delete d.agents[paneId];
    delete d.agentActivity[paneId];
    delete d.agentBackgroundWork[paneId];
    delete d.agentSubagents[paneId];
}

export function pruneWindowViews(d: StoreState, win: Window): void {
    for (const p of collectPanes(win.root)) {
        disposePaneState(d, p.id);
    }
}

export function closeDocument(win: Window, doc: string): void {
    // The editor owns the unsaved-changes prompt and the CodeMirror state for
    // each document, so closing goes through it rather than around it.
    if (win.role === "files") emit({ type: "close-file", paneId: editorPaneOf(win, getState().editorViews), path: doc });
    pluginDocuments(win.role)?.close(win.activePaneId, doc);
}
