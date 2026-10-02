import { pluginDocuments } from "../../plugins/documents";
import { taskPtyBindings } from "../../tasks/nativeRuntime";
import { invalidate } from "../resources";
import { getState, mutate } from "../store";
import { agentPaneId, editorPaneOf, nextInCycle, ownerSessionId, selectTabRefs, stripOrder, tabRefKey, type TabSource } from "../selectors";
import { collectPanes, cloneLayout, newId } from "../layout";
import type { Window, WindowRole, TabRef } from "../types";
import { setEditorView } from "./editor";
import { closeAgentDesk, closeDocument, dirtyPathsForWindow, guardDiscardDirty, makeWindow, pruneWindowViews, withActiveSession } from "./shared";

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
        const list = win.role === "files" ? d.editorViews[editorPaneOf(win, d.editorViews)]?.openTabs : undefined;
        if (list) moveBeside(list, sourceDoc, targetDoc, placement);
    });
    const win = getState().windows[windowId];
    if (win) pluginDocuments(win.role)?.reorder?.(win.activePaneId, sourceDoc, targetDoc, placement);
}

export function newWindow(): void {
    withActiveSession((d, session) => {
        const winIds = d.windowsBySession[session.id] ?? [];
        const w = makeWindow(session.cwd, "Terminal");
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
            closeAgentDesk(closingAgent.id);
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
    if (win.role === "files") setEditorView(editorPaneOf(win, getState().editorViews), { activePath: doc });
    pluginDocuments(win.role)?.select(win.activePaneId, doc);
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
