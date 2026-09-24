import { pluginDocuments } from "../plugins/documents";
import type { PluginManifest } from "../api/plugins";
import { fixedSessionName } from "./sessionNames";
import { isPluginKind, pluginIdOf, type PluginKind } from "../plugins/kinds";
import { RAIL_GROUP_ORDER, railGroupOf } from "./railGroups";
import { invokeCommand as invoke } from "../api/invoke";
import type { AgentSession } from "../api/agents";
import { browserApi } from "../api/browser";
import { filesApi } from "../api/files";
import { lsp } from "../api/lsp";
import { sshApi } from "../api/ssh";
import { checkForUpdateNow } from "../api/updater";
import { basename, dirname, isPathWithin } from "../lib/paths";
import { clampRailWidth, type RailEdge } from "../lib/railWidths";
import { MAX_AGENT_MODEL_LENGTH, normalizePermissionMode } from "../agentLaunch";
import { cloneTheme, DEFAULT_THEME_ID, type Theme } from "../themes";
import { sshStartup } from "../terminal/sshStartup";
import { taskPtyBindings, type TaskTerminalPresentationRequest } from "../tasks/nativeRuntime";
import { applyTheme, applyWindowOpacity, previewTheme, registerCustomThemes } from "../themes/bus";
import { applyTerminalFontSize, clampTerminalFontSize, DEFAULT_TERMINAL_FONT_SIZE } from "../terminal/fontSize";
import { applyChatTextScale, clampChatTextScale, DEFAULT_CHAT_TEXT_SCALE } from "../chat/textScale";
import { applyEditorTextScale, clampEditorTextScale, DEFAULT_EDITOR_TEXT_SCALE } from "../editor/textScale";
import { emit } from "./bus";
import { reduceAgentState } from "./agentStatus";
import { invalidate, peekResource } from "./resources";
import { agentSessionsR, projectRootsScanR } from "./resources.defs";
import { getState, mutate, setState, type StoreState } from "./store";
import { notify, reportError, swallow } from "./toast";
import { agentIdsWithLiveSessions } from "./agentLiveSessions";
import { confirmDialog } from "./dialog";
import { agentSupportsSkipPermissions } from "./commands/agentLogic";
import { agentDirectCommand, agentStartup } from "./commands/agentLaunchCommand";
import { parseSessionBundle } from "./sessionBundle";
import {
    activeAgentId,
    agentIdsOf,
    agentWindowId,
    agentPaneId,
    nextInCycle,
    ownerSessionId,
    selectTabRefs,
    shownBrowserPaneId,
    stripOrder,
    tabRefKey,
    type TabSource,
} from "./selectors";
import { agentWindow } from "./agentWindow";
import { DEFAULT_GIT_VIEW, DEFAULT_GLOBAL_SEARCH_VIEW } from "./types";
import { copyText, readClipboardText } from "../lib/clipboard";
import type { SettingsPageId } from "../settingsIndex";
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
    AgentEffort,
    AgentPermissionMode,
    AgentType,
    CliOpenRequest,
    CliOpenResult,
    CliOpenTarget,
    FocusDir,
    PickerMode,
    PaneKind,
    ProviderProfile,
    Session,
    SessionKind,
    SplitDir,
    Window,
    WindowRole,
    TabRef,
    DiffTarget,
} from "./types";

export { agentSupportsSkipPermissions } from "./commands/agentLogic";
export { agentDirectCommand, agentStartup } from "./commands/agentLaunchCommand";
export { mergePinnedIntoRoots, normaliseProjectRoots } from "./commands/settingsLogic";

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
        pinned: false,
        activeWindowId,
    };
}

function projectWindows(cwd: string): Window[] {
    return [makeWindow(cwd, "1", { role: "term" })];
}

/**
 * Focus the session's window for `role`, creating a closable one if it has
 * none. `seedEditorPath` pre-opens a file so a freshly created editor hydrates
 * from the store the same way a restored session does.
 */
function ensureRoleWindow(role: WindowRole, kind: PaneKind, name: string, seedEditorPath?: string): void {
    const st = getState();
    const session = st.sessions[st.activeSessionId];
    if (!session || session.kind !== "project") return;
    const existing = (st.windowsBySession[session.id] ?? []).find((id) => st.windows[id]?.role === role);
    if (existing) {
        selectWindowId(existing);
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

/**
 * Drops the fixed editor, diff and search tabs that older sessions were built
 * with. An editor holding open files is kept and merely made closable, so a
 * restored session does not lose its work.
 */
export function pruneOnDemandWindows(): void {
    mutate((d) => {
        for (const sid of d.sessionOrder) {
            const winIds = d.windowsBySession[sid] ?? [];
            let changed = false;

            for (const id of winIds) {
                const win = d.windows[id];
                if (!win) continue;
                const onDemand = win.role === "diff" || win.role === "search" || win.role === "files";
                if (!onDemand) continue;

                const keepsWork = win.role === "files" && collectPanes(win.root).some((pane) => (d.editorViews[pane.id]?.openTabs.length ?? 0) > 0);
                if (keepsWork) {
                    if (win.fixed) {
                        const { fixed: _fixed, ...rest } = win;
                        d.windows[id] = rest;
                        changed = true;
                    }
                    continue;
                }

                for (const pane of collectPanes(win.root)) {
                    delete d.editorViews[pane.id];
                    delete d.gitViews[pane.id];
                }
                delete d.windows[id];
                changed = true;
            }

            if (!changed) continue;
            const kept = winIds.filter((id) => d.windows[id]);
            d.windowsBySession[sid] = kept;
            const sess = d.sessions[sid];
            if (!kept.includes(sess.activeWindowId)) sess.activeWindowId = kept[0] ?? sess.activeWindowId;
        }
    });
}

function attachSession(d: StoreState, session: Session, windows: Window[]): void {
    d.sessions[session.id] = session;
    d.sessionOrder.push(session.id);
    for (const w of windows) d.windows[w.id] = w;
    d.windowsBySession[session.id] = windows.map((w) => w.id);
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

/**
 * Runs `proceed` once the user accepts losing the listed edits. Stays
 * synchronous when nothing is dirty so the common close path has no extra tick.
 */
function guardDiscardDirty(paths: string[], action: string, proceed: () => void): void {
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

        // The editor is opened on demand, so a project that has never shown one
        // gets it created here rather than failing the request.
        if (!(getState().windowsBySession[ownerId] ?? []).some((id) => getState().windows[id]?.role === "files")) {
            mutate((d) => {
                const w = makeWindow(owner.cwd, "editor", { kind: "editor", role: "files" });
                d.windows[w.id] = w;
                d.windowsBySession[ownerId] = [...(d.windowsBySession[ownerId] ?? []), w.id];
            });
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

function openSingletonPaneSession(kind: PluginKind): void {
    mutate((d) => {
        const existing = d.sessionOrder.map((id) => d.sessions[id]).find((s) => s.kind === kind);
        if (existing) {
            d.activeSessionId = existing.id;
            d.zoomedPaneId = null;
            return;
        }
        const title = fixedSessionName(kind) ?? kind;
        const win = makeWindow("", title, { kind, role: kind, fixed: true });
        attachSession(d as unknown as StoreState, makeSession(kind, title, "", win.id), [win]);
    });
}

export const openPluginSession = (kind: PluginKind): void => openSingletonPaneSession(kind);

export const setPluginManifests = (pluginManifests: readonly PluginManifest[]): void => setState({ pluginManifests });

/** Switching a plugin off also closes whatever of it is open, since nothing can reach it any more. */
export function setPluginEnabled(id: string, enabled: boolean): void {
    if (!enabled) {
        const st = getState();
        for (const sessionId of st.sessionOrder) {
            const kind = st.sessions[sessionId]?.kind;
            if (kind && isPluginKind(kind) && pluginIdOf(kind) === id) closeSessionNow(sessionId);
        }
    }
    setState((s) => ({ disabledPlugins: enabled ? s.disabledPlugins.filter((known) => known !== id) : [...new Set([...s.disabledPlugins, id])] }));
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

export function reorderSession(sourceId: string, targetId: string, placement: "before" | "after"): void {
    mutate((d) => {
        const source = d.sessions[sourceId];
        const target = d.sessions[targetId];
        if (!source || !target || sourceId === targetId || source.kind !== target.kind) return;

        const slots = d.sessionOrder.flatMap((id, index) => (d.sessions[id]?.kind === source.kind ? [index] : []));
        const ordered = slots.map((index) => d.sessionOrder[index]).filter((id) => id !== sourceId);
        const targetIndex = ordered.indexOf(targetId);
        if (targetIndex < 0) return;

        ordered.splice(targetIndex + (placement === "after" ? 1 : 0), 0, sourceId);
        if (slots.every((slot, index) => d.sessionOrder[slot] === ordered[index])) return;
        slots.forEach((slot, index) => {
            d.sessionOrder[slot] = ordered[index];
        });
    });
}

/** Moves `source` next to `target` in `list`; false when either is missing or nothing would change. */
function moveBeside<T>(list: T[], source: T, target: T, placement: "before" | "after"): boolean {
    if (source === target) return false;
    const from = list.indexOf(source);
    if (from < 0 || !list.includes(target)) return false;
    const next = list.filter((item) => item !== source);
    next.splice(next.indexOf(target) + (placement === "after" ? 1 : 0), 0, source);
    if (next.every((item, index) => item === list[index])) return false;
    list.splice(0, list.length, ...next);
    return true;
}

/* The strip's order is the session's window order, so moving a tab moves its
   window. It is saved with the rest of the workspace and so survives a restart. */
export function reorderWindowTab(sessionId: string, sourceWindowId: string, targetWindowId: string, placement: "before" | "after"): void {
    mutate((d) => {
        const order = d.windowsBySession[sessionId];
        if (order) moveBeside(order, sourceWindowId, targetWindowId, placement);
    });
}

/* A document tab only moves among the documents of its own window: they sit
   together in the strip because one window holds them all. Which list a window
   keeps them in mirrors `documentsOf`. */
export function reorderDocumentTab(windowId: string, sourceDoc: string, targetDoc: string, placement: "before" | "after"): void {
    mutate((d) => {
        const win = d.windows[windowId];
        if (!win) return;
        const list = win.role === "files" ? d.editorViews[win.activePaneId]?.openTabs : undefined;
        if (list) moveBeside(list, sourceDoc, targetDoc, placement);
    });
    const win = getState().windows[windowId];
    if (win) pluginDocuments(win.role)?.reorder?.(win.activePaneId, sourceDoc, targetDoc, placement);
}

export function closeSession(id: string): void {
    guardDiscardDirty(dirtyPathsForSession(getState(), id), "close session", () => closeSessionNow(id));
}

function closeSessionNow(id: string): void {
    const beforeClose = getState();
    const closingCwd = beforeClose.sessions[id]?.cwd;
    const closingAgentIds = agentIdsOf(beforeClose, id);
    const taskPaneIds = (beforeClose.windowsBySession[id] ?? []).flatMap((windowId) => {
        const window = beforeClose.windows[windowId];
        return window
            ? collectPanes(window.root)
                  .filter((pane) => pane.externalPty)
                  .map((pane) => pane.id)
            : [];
    });
    mutate((d) => {
        if (d.sessionOrder.length <= 1) return;
        const closed = d.sessions[id];
        if (!closed) return;
        const idx = d.sessionOrder.indexOf(id);
        const winIds = d.windowsBySession[id] ?? [];
        const isSshConfig = winIds.some((windowId) => d.windows[windowId]?.role === "ssh-config");

        for (const wid of winIds) {
            const w = d.windows[wid];
            if (w) {
                for (const p of collectPanes(w.root as unknown as Window["root"])) {
                    if (d.gitModal?.ownerPaneId === p.id) d.gitModal = null;
                    disposePaneState(d, p.id);
                }
            }
            delete d.windows[wid];
        }
        delete d.windowsBySession[id];
        delete d.globalSearchBySession[id];
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
    if (!getState().sessions[id]) {
        for (const paneId of taskPaneIds) taskPtyBindings.release(paneId);
        for (const agentId of closingAgentIds) {
            void browserApi.closeAgent(agentId).catch(reportError("close agent browser"));
        }
    }
    if (closingCwd) {
        const stillOpen = Object.values(getState().sessions).some((s) => s.cwd === closingCwd);
        if (!stillOpen) {
            filesApi.evict(closingCwd);
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

export function cycleSessionGroup(delta: number): void {
    mutate((d) => {
        const cur = d.sessions[d.activeSessionId];
        if (!cur) return;
        const groupOf = (id: string) => {
            const session = d.sessions[id];
            return session ? railGroupOf(session.kind, d.pluginManifests, d.disabledPlugins) : null;
        };
        const populated = RAIL_GROUP_ORDER.filter((group) => d.sessionOrder.some((id) => groupOf(id) === group));
        if (populated.length < 2) return;
        const curGroup = railGroupOf(cur.kind, d.pluginManifests, d.disabledPlugins);
        const curIdx = curGroup ? populated.indexOf(curGroup) : -1;
        if (curIdx === -1) return;
        const nextGroup = populated[(curIdx + delta + populated.length) % populated.length];
        const nextId = d.sessionOrder.find((id) => groupOf(id) === nextGroup);
        if (!nextId) return;
        d.activeSessionId = nextId;
        d.zoomedPaneId = null;
    });
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

export async function runBackgroundCommand(
    custom: import("../commands/registry").CustomCommand,
    cwdOverride?: string,
    failOnNonZero = false,
    sessionIdOverride?: string,
): Promise<{ code: number; output: string }> {
    const st = getState();
    const session = st.sessions[sessionIdOverride ?? st.activeSessionId];
    if (!session) throw new Error("No active session for project command.");
    const commandCwd = cwdOverride || session.cwd;
    const result = await invoke<{ code: number; output: string }>("run_background_command", {
        command: custom.command,
        cwd: commandCwd || null,
        env: {
            SIKEMUX_SESSION_ID: session.id,
            SIKEMUX_SESSION_NAME: session.name,
            SIKEMUX_SESSION_KIND: session.kind,
            SIKEMUX_PROJECT: session.kind === "project" ? commandCwd : "",
        },
    });
    const summary = result.output.trim() || `exit ${result.code}`;
    notify(result.code === 0 ? "success" : "error", `${custom.title}: ${summary}`);
    if (failOnNonZero && result.code !== 0) throw new Error(`${custom.title} failed: ${summary}`);
    return result;
}

export function runCustomCommand(custom: import("../commands/registry").CustomCommand, cwdOverride?: string): void {
    const st = getState();
    const session = st.sessions[st.activeSessionId];
    if (!session) return;
    const startup = custom.command;
    const commandCwd = cwdOverride || session.cwd;
    if (custom.placement === "background") {
        void runBackgroundCommand(custom, commandCwd).catch(reportError(custom.title));
        return;
    }
    if (custom.placement === "popup") {
        setState({
            commandPopup: {
                id: newId("popup"),
                title: custom.title,
                startup,
                cwd: commandCwd,
                context: {
                    sessionId: session.id,
                    sessionName: session.name,
                    sessionKind: session.kind,
                    ...(session.kind === "project" && commandCwd ? { project: commandCwd } : {}),
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
        const pane = makePane(commandCwd, { startup });
        pane.title = custom.title;
        if (custom.placement === "terminal") {
            const ids = d.windowsBySession[current.id] ?? [];
            const created = makeWindow(commandCwd, custom.title, { startup });
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
        d.zoomedPaneId = null;
    });
}

function taskTerminalContainsControl(value: string): boolean {
    for (let index = 0; index < value.length; index += 1) {
        const code = value.charCodeAt(index);
        if (code <= 31 || (code >= 127 && code <= 159)) return true;
    }
    return false;
}

function requireTaskTerminalText(name: string, value: string, maxLength: number): string {
    if (
        typeof value !== "string" ||
        value.length === 0 ||
        value.length > maxLength ||
        value.trim().length === 0 ||
        taskTerminalContainsControl(value)
    ) {
        throw new TypeError(`${name} must be bounded non-blank text without control characters`);
    }
    return value;
}

/**
 * Open or focus a transient task-output window and return its pane ID. PTY IDs,
 * commands, and environment values are intentionally absent from this state.
 */
export function openTaskTerminal(request: TaskTerminalPresentationRequest): string {
    const project = requireTaskTerminalText("task project", request.project, 4_096);
    const cwd = requireTaskTerminalText("task cwd", request.cwd, 4_096);
    const terminalKey = requireTaskTerminalText("task terminal key", request.terminalKey, 8_192);
    const label = requireTaskTerminalText("task label", request.label, 256);
    if (!isPathWithin(cwd, project)) throw new TypeError("task working directory must stay within its project");
    if (request.signal.aborted) throw request.signal.reason ?? new DOMException("Task terminal presentation was aborted", "AbortError");

    let paneId: string | null = null;
    mutate((d) => {
        const owner = d.sessionOrder.map((id) => d.sessions[id]).find((session) => session?.kind === "project" && session.cwd === project);
        if (!owner) return;
        const windowIds = d.windowsBySession[owner.id] ?? [];
        for (const windowId of windowIds) {
            const candidate = d.windows[windowId];
            if (!candidate?.transient) continue;
            const pane = collectPanes(candidate.root).find((item) => item.externalPty && item.taskTerminalKey === terminalKey);
            if (!pane) continue;
            pane.cwd = cwd;
            pane.title = label;
            candidate.name = label;
            candidate.activePaneId = pane.id;
            owner.activeWindowId = candidate.id;
            d.activeSessionId = owner.id;
            d.zoomedPaneId = null;
            paneId = pane.id;
            return;
        }

        const created = makeWindow(cwd, label, { role: "named" });
        created.transient = true;
        if (created.root.type !== "pane") return;
        created.root.title = label;
        created.root.externalPty = true;
        created.root.taskTerminalKey = terminalKey;
        d.windows[created.id] = created;
        d.windowsBySession[owner.id] = [...windowIds, created.id];
        owner.activeWindowId = created.id;
        d.activeSessionId = owner.id;
        d.zoomedPaneId = null;
        paneId = created.root.id;
    });
    if (!paneId) throw new Error("Task project is no longer open");
    return paneId;
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
    const windows = (state.windowsBySession[session.id] ?? []).map((id) => state.windows[id]).filter((w): w is Window => !!w && w.role !== "agent");
    const agents = agentIdsOf(state, session.id)
        .map((id) => state.agents[id])
        .filter((agent): agent is Agent => !!agent?.resumeId)
        .map(({ type, title, resumeId }) => ({ type, title, resumeId }));
    const payload = JSON.stringify({ format: "sikemux-session", version: 1, session, windows, agents }, (key, value) =>
        key === "secretVars" || key === "drafts" || key === "startup" || key === "baselineSessionIds" ? undefined : value,
    );
    await copyText(payload);
    notify("success", `Copied ${session.name} session bundle (secrets and startup commands stripped)`);
}

export async function importSessionFromClipboard(): Promise<void> {
    const raw = await readClipboardText();
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
            activeWindowId: importedWindows[0].id,
        };
        attachSession(d as unknown as StoreState, session, importedWindows);
        for (const row of bundle.agents) {
            const id = newId("agent");
            d.agents[id] = {
                id,
                type: row.type,
                title: row.title,
                resumeId: row.resumeId,
                startup: agentStartup(row.type, row.resumeId),
                directCommand: agentDirectCommand(row.type, row.resumeId),
                launchState: "dormant",
            };
            const win = agentWindow(d.agents[id], sourceCwd);
            d.windows[win.id] = win;
            d.windowsBySession[sessionId].push(win.id);
        }
    });
    notify("success", "Imported session as a safe, dormant copy");
}

function closeActivePane(): void {
    let taskPaneId: string | null = null;
    withActiveWindow((d, w, session) => {
        const closingPaneId = w.activePaneId;
        if (collectPanes(w.root).some((pane) => pane.id === closingPaneId && pane.externalPty)) taskPaneId = closingPaneId;
        if (d.gitModal?.ownerPaneId === closingPaneId) d.gitModal = null;
        const root = removePane(w.root, closingPaneId);
        if (root === null && w.fixed) return;
        d.zoomedPaneId = null;
        disposePaneState(d, closingPaneId);
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
    if (taskPaneId) taskPtyBindings.release(taskPaneId);
}

/* Nothing shows this agent's browser once its pane is gone, so the tabs stop
   being worth keeping — including the ones a restored pane never opened. */
function dropBrowserPaneState(d: StoreState, paneId: string): void {
    const agentId = d.browserPanes[paneId];
    delete d.browserPanes[paneId];
    delete d.browserRestores[paneId];
    if (agentId && !Object.values(d.browserPanes).includes(agentId)) delete d.browserStrips[agentId];
}

function disposePaneState(d: StoreState, paneId: string): void {
    emit({ type: "pane-closed", paneId });
    if (d.gitModal?.ownerPaneId === paneId) d.gitModal = null;
    delete d.editorViews[paneId];
    delete d.pendingEditorOpens[paneId];
    delete d.dirtyEditorPaths[paneId];
    delete d.gitViews[paneId];
    dropBrowserPaneState(d, paneId);
    delete d.terminalTitles[paneId];
    delete d.agents[paneId];
    delete d.agentActivity[paneId];
    delete d.agentBackgroundWork[paneId];
    delete d.agentSubagents[paneId];
}

function pruneWindowViews(d: StoreState, win: Window): void {
    for (const p of collectPanes(win.root)) {
        disposePaneState(d, p.id);
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
    d.zoomedPaneId = null;
}

function closeActiveTerminalTab(): void {
    withActiveSession((d, session) => {
        const closing = d.windows[session.activeWindowId];
        if (!closing || closing.role !== "term") return;

        const winIds = d.windowsBySession[session.id] ?? [];
        const termIds = winIds.filter((id) => d.windows[id]?.role === "term");
        // A command or ssh session is its terminal, so its last tab is replaced
        // rather than closed. A project can sit on no terminal at all.
        if (termIds.length <= 1 && winIds.length <= 1 && session.kind !== "project") {
            replaceWithFreshTerminalTab(d, session, closing);
            return;
        }

        const idx = winIds.indexOf(closing.id);
        const remaining = winIds.filter((id) => id !== closing.id);
        const isTerm = (id: string) => d.windows[id]?.role === "term";
        const before = remaining.slice(0, idx).reverse().find(isTerm);
        const after = remaining.slice(idx).find(isTerm);
        const nextId = before ?? after ?? remaining[Math.min(idx, remaining.length - 1)] ?? "";

        pruneWindowViews(d, closing);
        delete d.windows[closing.id];
        d.windowsBySession[session.id] = remaining;
        const sess = d.sessions[session.id];
        sess.activeWindowId = nextId;
        d.zoomedPaneId = null;
    });
}

export function closeActiveFocusTarget(): void {
    const st = getState();
    const session = st.sessions[st.activeSessionId];
    if (!session) return;

    // The agent picker is frontmost while open, so ⌥W dismisses it before it
    // reaches whatever is behind it.
    if (st.agentPaletteOpen) {
        closeAgentPalette();
        return;
    }

    const win = st.windows[session.activeWindowId];
    if (win?.role === "agent") {
        const paneId = agentPaneId(win);
        if (paneId) closeAgent(paneId);
        return;
    }
    if (win?.role === "ssh-config") {
        closeSession(session.id);
        return;
    }

    const documents = win ? pluginDocuments(win.role) : undefined;
    if (win && documents) {
        // ⌥W closes the document in front, not the plugin holding it.
        const { activeId } = documents.list(win.activePaneId);
        if (activeId) documents.close(win.activePaneId, activeId);
        return;
    }

    if (win && collectPanes(win.root).length > 1) {
        guardDiscardDirty(dirtyPathsForPane(st, win.activePaneId), "close pane", closeActivePane);
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
        const { panes } = computeLayout(w.root, w.activePaneId);
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
        d.zoomedPaneId = null;
    });
}

export function duplicateWindow(id: string): void {
    mutate((d) => {
        const source = d.windows[id];
        const ownerId = d.sessionOrder.find((sid) => d.windowsBySession[sid]?.includes(id));
        if (!source || !ownerId || source.role === "agent") return;
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
    });
}

export function closeWindowById(id: string): void {
    const st = getState();
    const closing = st.windows[id];
    if (!closing || closing.fixed) return;
    guardDiscardDirty(dirtyPathsForWindow(st, closing), "close window", () => closeWindowNow(id));
}

function closeWindowNow(id: string): void {
    const closing = getState().windows[id];
    if (!closing || closing.fixed) return;
    const taskPaneIds = collectPanes(closing.root)
        .filter((pane) => pane.externalPty)
        .map((pane) => pane.id);
    const closingAgent = closing.role === "agent" ? getState().agents[agentPaneId(closing) ?? ""] : undefined;
    mutate((d) => {
        const sessionId = ownerSessionId(d, id);
        const session = sessionId ? d.sessions[sessionId] : undefined;
        if (!session) return;
        const winIds = d.windowsBySession[session.id] ?? [];
        // A project may sit on zero windows, showing agents or nothing until a
        // tab is opened. Other session kinds are their window, so keep one.
        if (winIds.length <= 1 && session.kind !== "project") return;
        const closing = d.windows[id];
        if (!closing || closing.fixed) return;
        const idx = winIds.indexOf(id);
        const remaining = winIds.filter((wid) => wid !== id);
        const sess = d.sessions[session.id];
        if (sess.activeWindowId === id) {
            let nextId = remaining[Math.min(idx, remaining.length - 1)] ?? "";
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
    if (!getState().windows[id]) {
        for (const paneId of taskPaneIds) taskPtyBindings.release(paneId);
        if (closingAgent) {
            void browserApi.closeAgent(closingAgent.id).catch(reportError("close agent browser"));
            if (closingAgent.type === "claude" || closingAgent.type === "codex") {
                invalidate((kind) => kind === "agents.catalog" || kind === "agents.models" || kind === "agents.usage");
            }
        }
    }
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
        if (sess.activeWindowId === id && d.zoomedPaneId === null) {
            return;
        }
        sess.activeWindowId = id;
        d.zoomedPaneId = null;
    });
}

/** Shows `doc` in the window holding it; the window's role says which view keeps it. */
function selectDocument(win: Window, doc: string): void {
    if (win.role === "files") setEditorView(win.activePaneId, { activePath: doc });
    pluginDocuments(win.role)?.select(win.activePaneId, doc);
}

function closeDocument(win: Window, doc: string): void {
    // The editor owns the unsaved-changes prompt and the CodeMirror state for
    // each document, so closing goes through it rather than around it.
    if (win.role === "files") emit({ type: "close-file", paneId: win.activePaneId, path: doc });
    pluginDocuments(win.role)?.close(win.activePaneId, doc);
}

export function selectTab(ref: TabRef): void {
    const win = getState().windows[ref.id];
    if (win && ref.doc !== undefined) selectDocument(win, ref.doc);
    selectWindowId(ref.id);
}

export function closeTab(ref: TabRef): void {
    const win = getState().windows[ref.id];
    if (win && ref.doc !== undefined) {
        closeDocument(win, ref.doc);
        return;
    }
    closeWindowById(ref.id);
}

/**
 * The id `delta` steps to, or null when the strip would not move.
 *
 * Landing back on the active tab is not a move, and re-selecting it would
 * rebuild view state for no reason, so it reads as nothing to do.
 */
function nextTabIn(source: TabSource, delta: number): string | null {
    const order = stripOrder(getState(), source);
    const next = nextInCycle(order, delta);
    return next === order.activeId ? null : next;
}

export function cycleTab(delta: number): void {
    const st = getState();
    const sessionId = st.activeSessionId;
    if (!st.sessions[sessionId]) return;
    const nextKey = nextTabIn({ kind: "workspace", sessionId }, delta);
    const next = nextKey ? selectTabRefs(st, sessionId).find((ref) => tabRefKey(ref) === nextKey) : undefined;
    if (next) selectTab(next);
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

/** ⌥./⌥, — cycle whichever tab strip is currently on screen: agent tabs, terminal
 *  tabs, or the focused editor pane's open file tabs. */
export function cycleTabs(delta: number): void {
    const st = getState();
    const session = st.sessions[st.activeSessionId];
    if (!session) return;

    const win = st.windows[session.activeWindowId];
    if (!win) return;

    if (win.role === "agent") {
        const next = nextTabIn({ kind: "agents", sessionId: session.id }, delta);
        if (next) selectWindowId(next);
        return;
    }

    const documents = pluginDocuments(win.role);
    if (documents) {
        const order = documents.list(win.activePaneId);
        const next = nextInCycle(order, delta);
        if (next && next !== order.activeId) documents.select(win.activePaneId, next);
        return;
    }

    if (win.role === "term") {
        const next = nextTabIn({ kind: "terminals", sessionId: session.id }, delta);
        if (next) selectWindowId(next);
        return;
    }

    const pane = collectPanes(win.root).find((p) => p.id === win.activePaneId);
    if (pane?.kind !== "editor") return;
    const next = nextTabIn({ kind: "documents", paneId: pane.id }, delta);
    if (next) setEditorView(pane.id, { activePath: next });
}

const FALLBACK_AGENT_TITLE_MAX = 13;

function profileLaunchOptions(profile: ProviderProfile | undefined, model?: string, effort?: AgentEffort) {
    return {
        model,
        effort,
        configPath: profile?.configPath,
        environmentKeys: profile?.environmentKeys,
    };
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

export function configureEmptyAgent(id: string, type: "codex" | "claude", profileId?: string): void {
    mutate((d) => {
        const agent = d.agents[id];
        const activity = d.agentActivity[id];
        if (!agent || activity?.backendState === "working" || activity?.backendState === "blocked") return;
        const profile = profileId ? d.providerProfiles.find((item) => item.id === profileId && item.provider === type) : undefined;
        if (profileId && !profile) return;
        const mode = normalizePermissionMode(type, agent.permissionMode ?? d.defaultAgentPermissionMode);
        agent.type = type;
        agent.profileId = profile?.id;
        agent.title = profile?.name || type;
        agent.executablePath = profile?.executablePath;
        agent.permissionMode = mode;
        agent.skipPermissions = mode === "bypass";
        delete agent.resumeId;
        delete agent.model;
        delete agent.effort;
        delete agent.baselineSessionIds;
        const options = profileLaunchOptions(profile);
        agent.startup = agentStartup(type, undefined, mode, profile?.executablePath, options);
        agent.directCommand = agentDirectCommand(type, undefined, mode, profile?.executablePath, options);
    });
}

export function setAgentModelPreferences(id: string, model: string | undefined, effort: AgentEffort | undefined): void {
    mutate((d) => {
        const agent = d.agents[id];
        if (!agent) return;
        agent.model = model;
        agent.effort = effort;
        const profile = d.providerProfiles.find((item) => item.id === agent.profileId && item.provider === agent.type);
        const options = profileLaunchOptions(profile, model, effort);
        const executable = profile?.executablePath || agent.executablePath;
        const mode = agent.permissionMode ?? (agent.skipPermissions ? "bypass" : "workspace-write");
        agent.startup = agentStartup(agent.type, agent.resumeId, mode, executable, options);
        agent.directCommand = agentDirectCommand(agent.type, agent.resumeId, mode, executable, options);
    });
}

export function setAgentPermissionMode(id: string, requestedMode: AgentPermissionMode): void {
    mutate((d) => {
        const currentAgent = d.agents[id];
        const profile = currentAgent?.profileId
            ? d.providerProfiles.find((item) => item.id === currentAgent.profileId && item.provider === currentAgent.type)
            : undefined;
        const a = d.agents[id];
        if (!a) return;
        const next = normalizePermissionMode(a.type, requestedMode);
        const current = a.permissionMode ?? (a.skipPermissions ? "bypass" : normalizePermissionMode(a.type, "workspace-write"));
        if (next === current) return;
        a.permissionMode = next;
        a.skipPermissions = next === "bypass";
        const launchOptions = profileLaunchOptions(profile, a.model, a.effort);
        const executablePath = profile?.executablePath || a.executablePath;
        a.startup = agentStartup(a.type, a.resumeId, next, executablePath, launchOptions);
        a.directCommand = agentDirectCommand(a.type, a.resumeId, next, executablePath, launchOptions);
    });
}

export function toggleAgentSkipPermissions(id: string): void {
    const agent = getState().agents[id];
    if (!agent || !agentSupportsSkipPermissions(agent.type)) return;
    const current = agent.permissionMode ?? (agent.skipPermissions ? "bypass" : normalizePermissionMode(agent.type, "workspace-write"));
    const next = current === "bypass" ? normalizePermissionMode(agent.type, "workspace-write") : "bypass";
    setAgentPermissionMode(id, next);
}

/** ⌥Y — toggle YOLO (skip-permissions) for the active agent, when one is on screen. */
export function toggleActiveAgentSkipPermissions(): void {
    const st = getState();
    const session = st.sessions[st.activeSessionId];
    const id = activeAgentId(st, session);
    if (id) toggleAgentSkipPermissions(id);
}

export interface AddAgentOptions {
    permissionMode?: AgentPermissionMode;
    profileId?: string | null;
    model?: string;
    effort?: AgentEffort;
    baselineSessionIds?: string[];
    cwd?: string;
    /** Pin launches to the project that opened the picker. */
    sessionId?: string;
    detectedExecutablePath?: string;
}

export function addAgent(type: AgentType, resumeId?: string, title?: string, options: AddAgentOptions = {}): boolean {
    if ((options.model?.trim().length ?? 0) > MAX_AGENT_MODEL_LENGTH) return false;
    let attached = false;
    mutate((d) => {
        const session = d.sessions[options.sessionId ?? d.activeSessionId];
        if (!session) return;
        if (session.kind !== "project") return;
        const existing = resumeId
            ? agentIdsOf(d, session.id)
                  .map((id) => d.agents[id])
                  .find((a) => a && a.type === type && a.resumeId === resumeId)
            : undefined;
        const sess = d.sessions[session.id];
        d.zoomedPaneId = null;
        // A successful launch closes the picker and activates the new PTY.
        d.agentPaletteOpen = false;
        if (existing) {
            const winId = agentWindowId(d, existing.id);
            if (winId) sess.activeWindowId = winId;
            attached = true;
            return;
        }
        const permissionMode = normalizePermissionMode(type, options.permissionMode ?? d.defaultAgentPermissionMode);
        const requestedProfileId = options.profileId === undefined ? d.selectedProviderProfileIds[type] : options.profileId;
        const profileId = requestedProfileId
            ? d.providerProfiles.find((profile) => profile.id === requestedProfileId && profile.provider === type)?.id
            : undefined;
        const cwd = options.cwd || session.cwd;
        const model = options.model?.trim() || undefined;
        const profile = profileId ? d.providerProfiles.find((item) => item.id === profileId) : undefined;
        const executablePath = profile?.executablePath || options.detectedExecutablePath;
        const launchOptions = profileLaunchOptions(profile, model, options.effort);
        const agent: Agent = {
            id: newId("agent"),
            type,
            title: title ?? type,
            startup: agentStartup(type, resumeId, permissionMode, executablePath, launchOptions),
            directCommand: agentDirectCommand(type, resumeId, permissionMode, executablePath, launchOptions),
            resumeId,
            createdAt: Date.now(),
            permissionMode,
            profileId,
            executablePath,
            cwd,
            model,
            effort: options.effort,
            ...(permissionMode === "bypass" ? { skipPermissions: true } : {}),
            launchState: "live",
        };
        // Fresh agents (no resumeId) record the sessions that already exist so
        // reconciliation never adopts the session you were just in. The rail
        // keeps this list warm; on a cold cache we fall back to an mtime check.
        if (!resumeId) {
            const known = options.baselineSessionIds ?? peekResource(agentSessionsR, type, cwd, profile?.configPath)?.map((row) => row.id);
            if (known) agent.baselineSessionIds = [...new Set(known)];
        }
        d.agents[agent.id] = agent;
        const win = agentWindow(agent, cwd);
        d.windows[win.id] = win;
        d.windowsBySession[session.id] = [...(d.windowsBySession[session.id] ?? []), win.id];
        sess.activeWindowId = win.id;
        attached = true;
    });
    return attached;
}

export function reconcileAgentSessions(type: AgentType, cwd: string, configPath: string | undefined, rows: AgentSession[]): void {
    if (rows.length === 0) return;
    mutate((d) => {
        const rowById = new Map(rows.map((row) => [row.id, row]));
        const matchingAgents: Agent[] = [];
        for (const sessionId of d.sessionOrder) {
            const session = d.sessions[sessionId];
            if (session?.kind !== "project") continue;
            for (const agentId of agentIdsOf(d, sessionId)) {
                const agent = d.agents[agentId];
                const agentConfigPath = agent?.profileId
                    ? d.providerProfiles.find((profile) => profile.id === agent.profileId && profile.provider === agent.type)?.configPath
                    : undefined;
                if (agent?.type === type && (agent.cwd || session.cwd) === cwd && agentConfigPath === configPath) matchingAgents.push(agent);
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
                const winId = agentWindowId(d, agent.id);
                if (winId) d.windows[winId].name = nextTitle;
            }
        }

        const candidates = rows.filter((row) => !claimed.has(row.id)).sort((a, b) => b.mtime - a.mtime);
        if (candidates.length === 0) return;

        const freshAgents = matchingAgents
            .filter((agent) => !agent.resumeId && d.agentActivity[agent.id]?.source !== "acp")
            .sort((a, b) => (b.createdAt ?? 0) - (a.createdAt ?? 0));
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
            const profile = agent.profileId
                ? d.providerProfiles.find((item) => item.id === agent.profileId && item.provider === agent.type)
                : undefined;
            const launchOptions = profileLaunchOptions(profile, agent.model, agent.effort);
            agent.startup = agentStartup(
                agent.type,
                agent.resumeId,
                agent.permissionMode ?? (agent.skipPermissions ? "bypass" : "workspace-write"),
                profile?.executablePath || agent.executablePath,
                launchOptions,
            );
            agent.directCommand = agentDirectCommand(
                agent.type,
                agent.resumeId,
                agent.permissionMode ?? (agent.skipPermissions ? "bypass" : "workspace-write"),
                profile?.executablePath || agent.executablePath,
                launchOptions,
            );
            delete agent.baselineSessionIds;
            claimed.add(row.id);
        }
    });
}

export function attachAgentSession(id: string, resumeId: string): void {
    if (!resumeId.trim() || resumeId.length > 4_096 || /[\0\r\n]/.test(resumeId)) return;
    mutate((d) => {
        const agent = d.agents[id];
        if (!agent || agent.resumeId === resumeId) return;
        const profile = agent.profileId
            ? d.providerProfiles.find((candidate) => candidate.id === agent.profileId && candidate.provider === agent.type)
            : undefined;
        const launchOptions = profileLaunchOptions(profile, agent.model, agent.effort);
        const executablePath = profile?.executablePath || agent.executablePath;
        agent.resumeId = resumeId;
        agent.startup = agentStartup(
            agent.type,
            resumeId,
            agent.permissionMode ?? (agent.skipPermissions ? "bypass" : "workspace-write"),
            executablePath,
            launchOptions,
        );
        agent.directCommand = agentDirectCommand(
            agent.type,
            resumeId,
            agent.permissionMode ?? (agent.skipPermissions ? "bypass" : "workspace-write"),
            executablePath,
            launchOptions,
        );
        delete agent.baselineSessionIds;
    });
}

export function setAgentTitle(id: string, title: string): void {
    const value = title.trim();
    if (!value || value.length > 200 || /[\0\r\n]/.test(value)) return;
    mutate((d) => {
        const agent = d.agents[id];
        if (agent) agent.title = value;
    });
}

export function selectAgent(id: string): void {
    withActiveSession((d, session) => {
        const agent = d.agents[id];
        const winId = agentWindowId(d, id);
        if (!agent || !winId || !(d.windowsBySession[session.id] ?? []).includes(winId)) return;
        const sess = d.sessions[session.id];
        sess.activeWindowId = winId;
        // Picking a real agent tab replaces the draft, exactly like any other tab.
        d.agentPaletteOpen = false;
        if (agent.launchState === "dormant") {
            agent.launchState = "live";
            delete d.agentActivity[id];
            return;
        }
        const activity = d.agentActivity[id];
        if (activity) {
            activity.unread = false;
            if (activity.state === "done") activity.state = "idle";
        }
    });
}

/** Open an agent that lives in some other project, switching to it on the way. */
export function revealAgent(id: string): void {
    const state = getState();
    const windowId = agentWindowId(state, id);
    const sessionId = windowId ? ownerSessionId(state, windowId) : null;
    if (!sessionId) return;
    if (sessionId !== state.activeSessionId) selectSession(sessionId);
    selectAgent(id);
}

export function resumeAgent(id: string): void {
    mutate((d) => {
        const agent = d.agents[id];
        if (!agent) return;
        agent.launchState = "live";
        delete d.agentActivity[id];
    });
}

/* A turn is over long before the work it started is. Shells, monitors and
   subagents outlive the answer that launched them, and ending the agent ends
   them too, so the count of what is still going decides whether it can sleep. */
export function noteAgentBackgroundWork(id: string, tasks: number, subagents: number): void {
    mutate((d) => {
        if (!d.agents[id]) return;
        const count = tasks + subagents;
        if (count > 0) d.agentBackgroundWork[id] = count;
        else delete d.agentBackgroundWork[id];
        if (subagents > 0) d.agentSubagents[id] = subagents;
        else delete d.agentSubagents[id];
    });
}

export function agentHasBackgroundWork(state: StoreState, id: string): boolean {
    return (state.agentBackgroundWork[id] ?? 0) > 0;
}

export function sleepAgents(ids: readonly string[]): string[] {
    const sleeping = new Set(ids);
    const slept: string[] = [];
    mutate((d) => {
        for (const id of sleeping) {
            const agent = d.agents[id];
            if (!agent?.resumeId || agent.launchState === "dormant") continue;
            agent.launchState = "dormant";
            delete d.agentBackgroundWork[id];
            delete d.agentSubagents[id];
            slept.push(id);
        }
    });
    return slept;
}

export function sleepAgent(id: string): boolean {
    const agent = getState().agents[id];
    if (!agent?.resumeId) {
        notify("info", "This agent is still establishing its resumable session");
        return false;
    }
    return sleepAgents([id]).length === 1;
}

export function setAgentKeepAlive(id: string, keepAlive: boolean): void {
    mutate((d) => {
        const agent = d.agents[id];
        if (!agent) return;
        if (keepAlive) agent.keepAlive = true;
        else delete agent.keepAlive;
    });
}

export async function sleepIdleAgents(): Promise<number> {
    const state = getState();
    const ids = Object.values(state.agents)
        .filter(
            (agent) =>
                agent.launchState !== "dormant" &&
                !!agent.resumeId &&
                !agent.keepAlive &&
                !agentHasBackgroundWork(state, agent.id) &&
                state.agentActivity[agent.id]?.backendState === "idle",
        )
        .map((agent) => agent.id);
    const live = await agentIdsWithLiveSessions(state, ids);
    const count = sleepAgents(ids.filter((id) => !live.has(id))).length;
    notify("info", count === 0 ? "No idle resumable agents to sleep" : `Put ${count} idle agent${count === 1 ? "" : "s"} to sleep`);
    return count;
}

export function noteAcpAgentState(id: string, state: import("./types").AgentBackendState): void {
    noteAgentActivity(id, {
        agentId: id,
        state,
        sequence: (getState().agentActivity[id]?.sequence ?? 0) + 1,
        source: "acp",
        confidence: "high",
        reason: "ACP session state",
    });
}

export function noteAgentActivity(id: string, event: "working" | "complete" | import("./agentStatus").AgentStateEvent): void {
    mutate((d) => {
        if (!d.agents[id]) return;
        const visible = activeAgentId(d, d.sessions[d.activeSessionId]) === id;
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

/** An agent closes as its window does; the window's close handles its browser. */
export function closeAgent(id: string): void {
    const winId = agentWindowId(getState(), id);
    if (winId) closeWindowById(winId);
}

export function focusAgents(): void {
    // Agents only exist in project sessions. Other groups (plugins,
    // ssh, command) have no agents and no way back out of "agent" view, so the
    // The agent pane shortcut (⌥4) is a no-op there.
    if (getState().sessions[getState().activeSessionId]?.kind !== "project") return;
    withActiveSession((d, session) => {
        const sess = d.sessions[session.id];
        d.agentRailOpen = true;
        d.zoomedPaneId = null;
        if (d.windows[sess.activeWindowId]?.role === "agent") return;
        const first = (d.windowsBySession[session.id] ?? []).find((id) => d.windows[id]?.role === "agent");
        if (first) sess.activeWindowId = first;
        else d.agentPaletteOpen = true;
    });
    emit({ type: "agent-focus", sessionId: getState().activeSessionId });
}

export const setHome = (home: string): void => setState({ home });
export const setLastSessionId = (id: string): void => setState({ lastSessionId: id });
export const setTerminalTitle = (paneId: string, title: string): void =>
    setState((s) => ({ terminalTitles: { ...s.terminalTitles, [paneId]: title } }));
export const openPicker = (mode: PickerMode = "all"): void => setState({ pickerOpen: true, pickerMode: mode });
export const closePicker = (): void => setState({ pickerOpen: false });
// The agent picker is project-scoped and opens over the agent view.
export const openAgentPalette = (): void => {
    invalidate((kind) => kind === "agents.catalog" || kind === "agents.models" || kind === "agents.usage");
    mutate((d) => {
        const session = d.sessions[d.activeSessionId];
        if (session?.kind !== "project") return;
        d.agentPaletteOpen = true;
        d.zoomedPaneId = null;
    });
};
export const closeAgentPalette = (): void => {
    const state = getState();
    const session = state.sessions[state.activeSessionId];
    // A project with nothing open keeps the picker, or it would show a blank stage.
    if (session?.kind === "project" && (state.windowsBySession[session.id] ?? []).length === 0) return;
    setState({ agentPaletteOpen: false });
};
export const forceCloseAgentPalette = (): void => setState({ agentPaletteOpen: false });
export const openCommandPalette = (): void => setState({ commandPaletteOpen: true });
export const closeCommandPalette = (): void => setState({ commandPaletteOpen: false });
export const toggleCommandPalette = (): void => setState((s) => ({ commandPaletteOpen: !s.commandPaletteOpen }));
export const openOnboarding = (): void => setState({ onboardingOpen: true, diagnosticsOpen: false, whatsNewOpen: false });
export const closeOnboarding = (complete = true): void => setState({ onboardingOpen: false, ...(complete ? { onboardingComplete: true } : {}) });
export const openDiagnostics = (): void => setState({ diagnosticsOpen: true, onboardingOpen: false, whatsNewOpen: false });
export const closeDiagnostics = (): void => setState({ diagnosticsOpen: false });
export const openWhatsNew = (): void => setState({ whatsNewOpen: true, onboardingOpen: false, diagnosticsOpen: false });
export const closeWhatsNew = (): void =>
    setState((s) => ({ whatsNewOpen: false, lastSeenVersion: s.lastReleaseNotes?.version ?? s.lastSeenVersion }));
export const openNewTabPalette = (): void =>
    setState({ newTabPaletteOpen: true, filePaletteOpen: false, agentPaletteOpen: false, pickerOpen: false });
export const closeNewTabPalette = (): void => setState({ newTabPaletteOpen: false });

export const openFilePalette = (): void => setState({ filePaletteOpen: true });
export const closeFilePalette = (): void => setState({ filePaletteOpen: false });
export const openSettings = (page?: SettingsPageId): void => setState(page ? { settingsOpen: true, settingsPage: page } : { settingsOpen: true });
export const setSettingsPage = (page: SettingsPageId): void => setState({ settingsPage: page });
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

        const editorView = d.editorViews[editorPane.id] ?? { openTabs: [], activePath: null };
        if (!editorView.openTabs.includes(configPath)) editorView.openTabs.push(configPath);
        editorView.activePath = configPath;
        d.editorViews[editorPane.id] = editorView;

        configSession.activeWindowId = target.id;
        target.activePaneId = editorPane.id;
        d.zoomedPaneId = null;
        d.settingsOpen = false;
    });
}
// Focus mode hides both rails, so asking for one back has to leave focus mode.
export const toggleSideRail = (): void => setState((s) => (s.zenMode ? { zenMode: false, sideRailOpen: true } : { sideRailOpen: !s.sideRailOpen }));
export const toggleAgentRail = (): void =>
    setState((s) => (s.zenMode ? { zenMode: false, agentRailOpen: true } : { agentRailOpen: !s.agentRailOpen }));
export const setRailWidth = (edge: RailEdge, px: number): void =>
    setState(edge === "start" ? { sideRailWidth: clampRailWidth(edge, px) } : { agentRailWidth: clampRailWidth(edge, px) });
export const toggleZen = (): void => setState((s) => ({ zenMode: !s.zenMode }));

export function requestOpenFile(path: string, line?: number, character?: number): void {
    ensureRoleWindow("files", "editor", "editor", path);
    emit({ type: "open-file", path, line, character });
}

export const openEditorPane = (): void => ensureRoleWindow("files", "editor", "editor");
export const openDiffPane = (): void => ensureRoleWindow("diff", "diff", "diff");

export const openGitWorkbench = (): void => ensureRoleWindow("git", "git", "Git");

export function openGitPane(): void {
    openGitWorkbench();
}

function focusDiff(target: DiffTarget): void {
    const st = getState();
    const session = st.sessions[st.activeSessionId];
    if (!session) return;
    setState((state) => ({ diffTarget: { ...state.diffTarget, [session.cwd]: target } }));
    ensureRoleWindow("diff", "diff", "diff");
}

/** Review one changed file in the diff tab. `path` is repo-relative. */
export const openDiff = (path: string): void => focusDiff({ kind: "worktree", path });

/** Review a whole commit in the diff tab. */
export const openCommitDiff = (rev: string, subject: string): void => focusDiff({ kind: "commit", rev, subject });

export function setThemeId(id: string): void {
    applyTheme(id);
    setState({ themeId: id });
}

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

export function setTerminalFontSize(v: number): void {
    const value = clampTerminalFontSize(v);
    applyTerminalFontSize(value);
    setState({ terminalFontSize: value });
}

export function adjustTerminalFontSize(step: number): void {
    setTerminalFontSize(getState().terminalFontSize + step);
}

export function resetTerminalFontSize(): void {
    setTerminalFontSize(DEFAULT_TERMINAL_FONT_SIZE);
}

export function setChatTextScale(v: number): void {
    const value = clampChatTextScale(v);
    applyChatTextScale(value);
    setState({ chatTextScale: value });
}

export function adjustChatTextScale(step: number): void {
    setChatTextScale(getState().chatTextScale + step);
}

export function resetChatTextScale(): void {
    setChatTextScale(DEFAULT_CHAT_TEXT_SCALE);
}

export function setEditorTextScale(v: number): void {
    const value = clampEditorTextScale(v);
    applyEditorTextScale(value);
    setState({ editorTextScale: value });
}

export function adjustEditorTextScale(step: number): void {
    setEditorTextScale(getState().editorTextScale + step);
}

export function resetEditorTextScale(): void {
    setEditorTextScale(DEFAULT_EDITOR_TEXT_SCALE);
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

function activeBrowserAgentId(): string | null {
    const st = getState();
    const session = st.sessions[st.activeSessionId];
    if (session?.kind !== "project") return null;
    return activeAgentId(st, session);
}

export function newBrowserTab(forAgentId?: string): boolean {
    const agentId = forAgentId ?? activeBrowserAgentId();
    if (!agentId) return false;
    openBrowserPane(agentId);
    void browserApi.newTab(agentId).catch(reportError("open browser tab"));
    return true;
}

export function openUrlInBrowserPane(agentId: string, url: string): void {
    openBrowserPane(agentId);
    void browserApi.newTab(agentId, url).catch(reportError("open link in browser"));
}

/** Hiding the browser keeps its tabs alive, so showing it again brings them back as they were. */
export function toggleBrowserPane(agentId: string): void {
    const openPaneId = shownBrowserPaneId(getState(), agentId);
    if (openPaneId) {
        closeBrowserPane(openPaneId);
        return;
    }
    openBrowserPane(agentId);
    void browserApi
        .snapshot(agentId)
        .then((snapshot) => (snapshot.tabs.length === 0 ? browserApi.newTab(agentId) : undefined))
        .catch(reportError("open browser tab"));
}

/** Brings the browser on screen for an agent that started using it, leaving focus where the person had it. */
export function revealBrowserPane(agentId: string): void {
    if (shownBrowserPaneId(getState(), agentId)) return;
    openBrowserPane(agentId, { focus: false });
}

/**
 * Put the agent's browser beside it, once.
 *
 * The pane is a leaf like any other, so it splits, resizes and closes through
 * the layout rather than through anything the browser owns itself.
 */
export function openBrowserPane(agentId: string, opts: { focus?: boolean } = {}): void {
    const focus = opts.focus ?? true;
    mutate((d) => {
        const existing = Object.entries(d.browserPanes).find(([, owner]) => owner === agentId);
        const windowId = Object.keys(d.windows).find((id) => collectPanes(d.windows[id].root).some((pane) => pane.id === agentId));
        if (!windowId) return;
        const win = d.windows[windowId];
        if (existing && collectPanes(win.root).some((pane) => pane.id === existing[0])) {
            if (focus) win.activePaneId = existing[0];
            return;
        }
        const agentPane = collectPanes(win.root).find((candidate) => candidate.id === agentId);
        const pane = makePane(agentPane?.cwd ?? "", { kind: "browser" });
        win.root = splitPane(win.root, agentId, "row", pane);
        if (focus) win.activePaneId = pane.id;
        d.browserPanes[pane.id] = agentId;
        d.zoomedPaneId = null;
    });
}

/**
 * The last tab closed, or the pane came back from a layout without the agent
 * that gave it meaning — either way there is nothing left for it to show.
 */
export function closeBrowserPane(paneId: string): void {
    mutate((d) => {
        for (const id of Object.keys(d.windows)) {
            const win = d.windows[id];
            if (!collectPanes(win.root).some((pane) => pane.id === paneId)) continue;
            const root = removePane(win.root, paneId);
            if (root === null) return;
            win.root = root;
            const remaining = collectPanes(root);
            if (!remaining.some((pane) => pane.id === win.activePaneId)) win.activePaneId = remaining[0]?.id ?? win.activePaneId;
            d.zoomedPaneId = null;
        }
        dropBrowserPaneState(d, paneId);
    });
}

export function closeActiveBrowserTab(): boolean {
    const agentId = activeBrowserAgentId();
    if (!agentId) return false;
    void browserApi
        .snapshot(agentId)
        .then((snapshot) => (snapshot.activeTabId ? browserApi.closeTab(agentId, snapshot.activeTabId) : undefined))
        .catch(reportError("close browser tab"));
    return true;
}

export function cycleBrowserTab(delta: number): boolean {
    const agentId = activeBrowserAgentId();
    if (!agentId) return false;
    void browserApi
        .snapshot(agentId)
        .then((snapshot) => {
            if (snapshot.tabs.length < 2) return;
            const current = Math.max(
                0,
                snapshot.tabs.findIndex((tab) => tab.id === snapshot.activeTabId),
            );
            const next = (current + delta + snapshot.tabs.length) % snapshot.tabs.length;
            return browserApi.switchTab(agentId, snapshot.tabs[next].id);
        })
        .catch(reportError("switch browser tab"));
    return true;
}

export function reloadBrowserTab(): boolean {
    const agentId = activeBrowserAgentId();
    if (!agentId) return false;
    void browserApi.reload(agentId).catch(reportError("reload browser"));
    return true;
}

export function browserHistory(delta: number): boolean {
    const agentId = activeBrowserAgentId();
    if (!agentId) return false;
    void (delta < 0 ? browserApi.back(agentId) : browserApi.forward(agentId)).catch(reportError("navigate browser history"));
    return true;
}

export function focusBrowserAddress(): boolean {
    const agentId = activeBrowserAgentId();
    if (!agentId) return false;
    const selector = `.browser-pane[data-agent-id="${CSS.escape(agentId)}"] .browser-address`;
    const focus = () => {
        const input = document.querySelector<HTMLInputElement>(selector);
        input?.focus();
        input?.select();
    };
    if (document.querySelector(selector)) focus();
    else
        void browserApi
            .newTab(agentId)
            .then(() => window.setTimeout(focus, 50))
            .catch(reportError("open browser address"));
    return true;
}
export const setRestoreAgentTabs = (value: boolean): void => setState({ restoreAgentTabs: value });
export const setUiTextScale = (value: number): void => setState({ uiTextScale: [1, 1.1, 1.25].includes(value) ? value : 1 });

export const setRailDensity = (value: import("./types").RailDensity): void => setState({ railDensity: value });
export const setDefaultAgentPermissionMode = (value: import("./types").AgentPermissionMode): void =>
    setState({ defaultAgentPermissionMode: value === "bypass" ? "bypass" : "workspace-write" });
export function selectProviderProfile(type: AgentType, profileId: string): void {
    setState((state) => ({ selectedProviderProfileIds: { ...state.selectedProviderProfileIds, [type]: profileId } }));
}
export function saveProviderProfile(profile: import("./types").ProviderProfile): void {
    setState((state) => {
        const selectedProviderProfileIds = { ...state.selectedProviderProfileIds };
        for (const [type, selected] of Object.entries(selectedProviderProfileIds)) {
            if (selected === profile.id && type !== profile.provider) delete selectedProviderProfileIds[type as AgentType];
        }
        return {
            providerProfiles: [...state.providerProfiles.filter((item) => item.id !== profile.id), profile],
            selectedProviderProfileIds,
        };
    });
}
export function deleteProviderProfile(id: string): void {
    if (id.startsWith("builtin-")) return;
    setState((state) => {
        const selectedProviderProfileIds = { ...state.selectedProviderProfileIds };
        for (const [type, selected] of Object.entries(selectedProviderProfileIds)) {
            if (selected === id) delete selectedProviderProfileIds[type as AgentType];
        }
        return { providerProfiles: state.providerProfiles.filter((profile) => profile.id !== id), selectedProviderProfileIds };
    });
}
export const checkForUpdates = (): Promise<void> => checkForUpdateNow();

export const setUpdateChannel = (value: "stable" | "nightly"): void => {
    if (getState().updateChannel === value) return;
    // The other channel's result says nothing about this one, and waiting out
    // the 30-minute poll to learn what the new channel offers reads as broken.
    setState({ updateChannel: value, pendingUpdate: null, lastUpdateCheck: null });
    void checkForUpdateNow();
};

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

export function addProjectRoot(path: string, depth = 1, selfIndex = false): void {
    const boundedDepth = Math.max(0, Math.min(8, Math.round(Number.isFinite(depth) ? depth : 1)));
    setState((s) =>
        s.projectRoots.some((r) => r.path === path) ? {} : { projectRoots: [...s.projectRoots, { path, depth: boundedDepth, selfIndex }] },
    );
    invalidate((kind) => kind === projectRootsScanR.kind);
}

/** Index the folder itself as a project, on top of whatever its depth finds. */
export function setProjectRootSelfIndex(path: string, selfIndex: boolean): void {
    setState((s) => ({
        projectRoots: s.projectRoots.map((r) => (r.path === path ? { ...r, selfIndex } : r)),
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
    const d = Math.max(0, Math.min(8, Math.round(Number.isFinite(depth) ? depth : 1)));
    setState((s) => ({
        projectRoots: s.projectRoots.map((r) => (r.path === path ? { ...r, depth: d } : r)),
    }));
    invalidate((kind) => kind === projectRootsScanR.kind);
}

export function openEditorTab(paneId: string, path: string, activate = true): void {
    mutate((d) => {
        const cur = d.editorViews[paneId] ?? { openTabs: [], activePath: null };
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
    ensureRoleWindow("search", "search", "Search");
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
