import { pluginDocuments } from "../../plugins/documents";
import { taskPtyBindings } from "../../tasks/nativeRuntime";
import { sshStartup } from "../../terminal/sshStartup";
import { getState, mutate, type StoreState } from "../store";
import { agentPaneId, editorPaneOf, ownerSessionId, paneToSeparate, tabSplitAllowed, type SplitSide } from "../selectors";
import {
    collectPanes,
    computeLayout,
    makePane,
    neighborPane,
    newId,
    addBeside,
    removePane,
    resizeTowards,
    setSplitSizes as setSplitSizesFn,
    splitPane,
} from "../layout";
import type { FocusDir, PaneNode, Session, SplitDir, Window, WindowRole, TabRef } from "../types";
import { closeAgent } from "./agents";
import { requestOpenFile } from "./editor";
import { closeSession } from "./sessions";
import { closeDesk, closeShownDeskTab } from "./desk";
import {
    busyAgentIds,
    closeDocument,
    dirtyPathsForPane,
    disposePaneState,
    guardDiscardDirty,
    guardStopAgents,
    makeWindow,
    patchWindow,
    pruneWindowViews,
    withActiveSession,
    withActiveWindow,
} from "./shared";
import { selectWindowId } from "./tabs";
import { closeAgentPalette } from "./ui";

export function splitActivePane(dir: SplitDir): void {
    withActiveWindow((d, w, session) => {
        const np = makePane(session.cwd, session.kind === "ssh" ? { startup: sshStartup(session.name) } : {});
        const win = d.windows[w.id];
        if (!win) return;
        win.root = splitPane(w.root, w.activePaneId, dir, np);
        win.activePaneId = np.id;
        d.zoomedPaneId = null;
    });
}

/** The tab a pane is, when it comes back out of a split: the one it had before, or the kind of tab its content makes. */
function tabOfPane(pane: PaneNode): { name: string; role: WindowRole } {
    if (pane.tab) return pane.tab;
    if (pane.kind === "agent") return { name: pane.title, role: "agent" };
    if (pane.kind === "git" || pane.kind === "search") return { name: pane.title, role: pane.kind };
    return { name: pane.title, role: pane.startup ? "named" : "term" };
}

/** Where each pane's tab was in the strip before it was split into another, so moving it out puts it back. */
const splitPlaces = new Map<string, { windowId: string; previous: string | null; next: string | null }>();

/**
 * Puts the tab `source` beside the pane `besidePaneId` of the tab its session
 * is showing, on `side`: up to three across, or two stacked, sharing the space
 * evenly. The tab that stays is the agent's when there is one; the other one's
 * pane moves into it, still running. A file moves out of the editor into a
 * pane of its own.
 */
export function splitWithTab(sessionId: string, source: TabRef, side: SplitSide, besidePaneId?: string): void {
    if (!tabSplitAllowed(getState(), sessionId, source)) return;
    const editor = getState().windows[source.id];
    const shownId = getState().sessions[sessionId]?.activeWindowId;
    mutate((d) => {
        const session = d.sessions[sessionId];
        const shown = d.windows[session.activeWindowId];
        const from = d.windows[source.id];
        const moving = source.doc !== undefined ? makePane(session.cwd, { kind: "editor" }) : (from.root as PaneNode);
        const root = addBeside(shown.root, besidePaneId ?? null, moving, side);
        if (!root) return;
        const host = source.doc === undefined && from.role === "agent" ? from : shown;
        if (source.doc !== undefined) {
            d.editorViews[moving.id] = { openTabs: [source.doc], activePath: source.doc, single: true };
        } else {
            for (const win of [shown, from]) for (const pane of collectPanes(win.root)) pane.tab ??= { name: win.name, role: win.role };
            const leaving = host === from ? shown : from;
            const ids = d.windowsBySession[sessionId] ?? [];
            const index = ids.indexOf(leaving.id);
            const place = { windowId: leaving.id, previous: ids[index - 1] ?? null, next: ids[index + 1] ?? null };
            for (const pane of collectPanes(leaving.root)) splitPlaces.set(pane.id, place);
            delete d.windows[leaving.id];
            d.windowsBySession[sessionId] = ids.filter((id) => id !== leaving.id);
        }
        host.root = root;
        host.activePaneId = moving.id;
        session.activeWindowId = host.id;
        d.zoomedPaneId = null;
    });
    // Beside another tab the file moves out of the editor; beside the editor itself it is a second view of it.
    if (source.doc !== undefined && editor && editor.id !== shownId) closeDocument(editor, source.doc);
}

/**
 * Moves a pane out of a split tab (the focused one when unnamed): a terminal
 * back to its own tab, in the place and under the name it had before it was
 * split in, still running, and a file back into the editor. When the pane
 * leaving is the one the split tab was named after, the tab takes the name of
 * a pane it still holds.
 */
export function separatePane(windowId: string, paneId?: string): void {
    const st = getState();
    const win = st.windows[windowId];
    const pane = win ? paneToSeparate(win, st, paneId) : null;
    const sessionId = win ? ownerSessionId(st, windowId) : null;
    if (!win || !pane || !sessionId) return;
    const view = pane.kind === "editor" ? st.editorViews[pane.id] : undefined;
    // The file showing goes last, so the editor ends on it.
    const paths = view ? [...view.openTabs.filter((open) => open !== view.activePath), ...(view.activePath ? [view.activePath] : [])] : [];
    const place = splitPlaces.get(pane.id);
    splitPlaces.delete(pane.id);
    mutate((d) => {
        const host = d.windows[windowId];
        const rest = removePane(host.root, pane.id);
        if (!rest) return;
        host.root = rest;
        if (!collectPanes(rest).some((candidate) => candidate.id === host.activePaneId)) host.activePaneId = collectPanes(rest)[0].id;
        const namesHost = (candidate: PaneNode) => {
            const tab = tabOfPane(candidate);
            return tab.name === host.name && tab.role === host.role;
        };
        const remaining = collectPanes(rest).filter((candidate) => candidate.kind !== "editor");
        if (pane.kind !== "editor" && namesHost(pane) && remaining.length > 0 && !remaining.some(namesHost)) {
            const next = tabOfPane(remaining[0]);
            host.name = next.name;
            host.role = next.role;
        }
        if (rest.type === "pane") delete rest.tab;
        d.zoomedPaneId = null;
        if (pane.kind === "editor") {
            delete d.editorViews[pane.id];
            return;
        }
        const ids = d.windowsBySession[sessionId] ?? [];
        const separated: Window = {
            id: place && !d.windows[place.windowId] ? place.windowId : newId("win"),
            ...tabOfPane(pane),
            root: { ...pane, tab: undefined },
            activePaneId: pane.id,
        };
        // Beside the tab it sat after, or the one it sat before, whichever is still there.
        const at =
            place?.previous && ids.includes(place.previous)
                ? ids.indexOf(place.previous) + 1
                : place?.next && ids.includes(place.next)
                  ? ids.indexOf(place.next)
                  : place && place.previous === null
                    ? 0
                    : ids.indexOf(windowId) + 1;
        d.windows[separated.id] = separated;
        d.windowsBySession[sessionId] = [...ids.slice(0, at), separated.id, ...ids.slice(at)];
        d.sessions[sessionId].activeWindowId = separated.id;
    });
    for (const path of paths) requestOpenFile(path);
}

/** Takes a split tab apart: every pane that can leave goes back where it came from, and the agent, if any, keeps the tab. */
export function unsplitTab(windowId: string): void {
    const win = getState().windows[windowId];
    if (!win) return;
    for (const pane of collectPanes(win.root)) if (pane.kind !== "agent") separatePane(windowId, pane.id);
    const sessionId = ownerSessionId(getState(), windowId);
    if (sessionId && getState().windows[windowId]) mutate((d) => void (d.sessions[sessionId].activeWindowId = windowId));
}

/** Closes one pane of a split, leaving the rest of the tab open. */
export function closePane(windowId: string, paneId: string): void {
    selectWindowId(windowId);
    focusPane(paneId);
    guardDiscardDirty(dirtyPathsForPane(getState(), paneId), "close pane", closeActivePane);
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
    let taskPaneIds: string[] = [];
    withActiveSession((d, session) => {
        const closing = d.windows[session.activeWindowId];
        if (!closing || closing.role !== "term") return;
        taskPaneIds = collectPanes(closing.root)
            .filter((pane) => pane.externalPty)
            .map((pane) => pane.id);

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
    for (const paneId of taskPaneIds) taskPtyBindings.release(paneId);
}

export function closeActiveFocusTarget(): void {
    const st = getState();
    const session = st.sessions[st.activeSessionId];
    if (!session) return;

    // The agent picker is frontmost while open, so it goes before whatever is behind it.
    if (st.agentPaletteOpen) {
        closeAgentPalette();
        return;
    }

    const win = st.windows[session.activeWindowId];
    if (win?.role === "agent") {
        const agentId = agentPaneId(win);
        const active = collectPanes(win.root).find((pane) => pane.id === win.activePaneId);
        if (active?.kind === "desk") {
            if (!agentId || !closeShownDeskTab(agentId)) closeDesk(active.id);
            return;
        }
        if (active && active.id !== agentId) {
            guardDiscardDirty(dirtyPathsForPane(st, active.id), "close pane", closeActivePane);
            return;
        }
        if (agentId) guardStopAgents(busyAgentIds(st, [agentId]), `Close ${st.agents[agentId]?.title ?? "agent"}?`, () => closeAgent(agentId));
        return;
    }
    if (win?.role === "ssh-config") {
        closeSession(session.id);
        return;
    }

    const documents = win ? pluginDocuments(win.role) : undefined;
    if (win && documents) {
        // The document in front closes, not the plugin holding it.
        const { activeId } = documents.list(win.activePaneId);
        if (activeId) documents.close(win.activePaneId, activeId);
        return;
    }

    if (win?.role === "files" && collectPanes(win.root).length === 1) {
        const activePath = st.editorViews[editorPaneOf(win, st.editorViews)]?.activePath;
        if (activePath) {
            closeDocument(win, activePath);
            return;
        }
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

    if (win) guardDiscardDirty(dirtyPathsForPane(st, win.activePaneId), "close pane", closeActivePane);
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
