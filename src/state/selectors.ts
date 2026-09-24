import { pluginDocuments } from "../plugins/documents";
import type { PaneKind, Session, TabRef, Window } from "./types";
import type { StoreState } from "./store";
import { collectPanes } from "./layout";

export const selectSessionIds = (state: StoreState): readonly string[] => state.sessionOrder;
export const selectActiveSessionId = (state: StoreState): string => state.activeSessionId;
export const selectActiveSession = (state: StoreState): Session | undefined => state.sessions[state.activeSessionId];

export const selectSession =
    (sessionId: string) =>
    (state: StoreState): Session | undefined =>
        state.sessions[sessionId];
export const selectWindow =
    (windowId: string) =>
    (state: StoreState): Window | undefined =>
        state.windows[windowId];
export const selectAgent = (agentId: string) => (state: StoreState) => state.agents[agentId];

export const selectWindowIds =
    (sessionId: string) =>
    (state: StoreState): readonly string[] =>
        state.windowsBySession[sessionId] ?? EMPTY_IDS;
export interface AgentAttention {
    agentId: string;
    agentTitle: string;
    agentType: import("./types").AgentType;
    sessionId: string;
    sessionName: string;
}

/**
 * Every agent across every project that stopped to ask the user something. The
 * rail lists these together because a blocked agent in a project you are not
 * looking at is exactly the one you cannot see.
 */
export function agentsAwaitingInput(
    state: Pick<StoreState, "sessionOrder" | "sessions" | "windows" | "windowsBySession" | "agents" | "agentActivity">,
): AgentAttention[] {
    const waiting: AgentAttention[] = [];
    for (const sessionId of state.sessionOrder) {
        const session = state.sessions[sessionId];
        if (!session) continue;
        for (const agentId of agentIdsOf(state, sessionId)) {
            if (state.agentActivity[agentId]?.state !== "blocked") continue;
            const agent = state.agents[agentId];
            if (!agent) continue;
            waiting.push({ agentId, agentTitle: agent.title, agentType: agent.type, sessionId, sessionName: session.name });
        }
    }
    return waiting;
}

/**
 * The agents a session holds, in strip order. An agent is a window whose one
 * pane carries its id, so this is a read over the windows, not a second list.
 */
export function agentIdsOf(state: Pick<StoreState, "windowsBySession" | "windows">, sessionId: string): string[] {
    return (state.windowsBySession[sessionId] ?? EMPTY_IDS).flatMap((id) => {
        const win = state.windows[id];
        const agentId = win && win.role === "agent" ? agentPaneId(win) : null;
        return agentId ? [agentId] : [];
    });
}

/**
 * The agent's own pane in an agent window.
 *
 * Not `activePaneId`: an agent window can hold a second pane — its browser —
 * and focusing that one would otherwise lose the agent, along with its tab, its
 * rail row and its place in what gets persisted.
 */
export function agentPaneId(win: Window): string | null {
    return collectPanes(win.root).find((pane) => pane.kind === "agent")?.id ?? null;
}

/** The window an agent lives in, wherever it is. */
export function agentWindowId(state: Pick<StoreState, "windows">, agentId: string): string | null {
    for (const win of Object.values(state.windows)) if (win.role === "agent" && agentPaneId(win) === agentId) return win.id;
    return null;
}

/** The agent a session is looking at, if its active window is one. */
/** The agent's browser pane, when one is in a window's layout right now. */
export function shownBrowserPaneId(state: Pick<StoreState, "browserPanes" | "windows">, agentId: string): string | null {
    const paneId = Object.keys(state.browserPanes).find(
        (id) =>
            state.browserPanes[id] === agentId && Object.values(state.windows).some((win) => collectPanes(win.root).some((pane) => pane.id === id)),
    );
    return paneId ?? null;
}

export function activeAgentId(state: Pick<StoreState, "windows">, session: Pick<Session, "activeWindowId"> | undefined): string | null {
    const win = session ? state.windows[session.activeWindowId] : undefined;
    return win?.role === "agent" ? agentPaneId(win) : null;
}

/** The session a window belongs to. */
export function ownerSessionId(state: Pick<StoreState, "sessionOrder" | "windowsBySession">, windowId: string): string | null {
    return state.sessionOrder.find((sid) => state.windowsBySession[sid]?.includes(windowId)) ?? null;
}

/**
 * Roles the workspace rail drives, which therefore have no tab of their own.
 *
 * The rail is how you reach these and the stage is where they render, so a tab
 * for them was a second handle on one surface: "Git" in the rail and "Git" in
 * the strip both meant the same screen. Every other role keeps its tab, since
 * nothing else offers a way back to it.
 *
 * `files` is absent here because an editor is not one surface: the rail browses
 * the tree, but each open document is its own thing to switch between, so an
 * editor contributes a tab per document instead of none.
 */
const RAIL_DRIVEN_ROLES: ReadonlySet<string> = new Set(["diff", "search", "git"]);

/** Whether `role` contributes a window entry to the session tab strip. */
export function roleHasTab(role: string): boolean {
    return !RAIL_DRIVEN_ROLES.has(role);
}

/**
 * The documents a window holds, when its role holds any.
 *
 * This is the one place that knows which roles expand into a tab per
 * document and where each keeps its list, so a new document-holding kind is
 * a case here and nowhere else.
 */
export function documentsOf(win: Window, editorViews: StoreState["editorViews"]): { ids: readonly string[]; activeId: string | null } | null {
    if (win.role === "files") {
        const view = editorViews[win.activePaneId];
        return { ids: view?.openTabs ?? EMPTY_IDS, activeId: view?.activePath ?? null };
    }
    return pluginDocuments(win.role)?.list(win.activePaneId) ?? null;
}

/**
 * Expand one session's windows into strip entries.
 *
 * Rail-driven roles contribute nothing: the rail reaches them and the stage
 * renders them, so a tab would be a second handle on one surface. An editor,
 * and any plugin surface that holds documents, contributes one entry per open
 * document, which is what puts them in this strip rather than a second bar
 * inside the pane; with nothing open they contribute nothing, because an empty
 * one is not worth a tab. Everything else gets exactly one entry, and the list
 * is derived rather than stored, so a window can never exist without its tab.
 */
export function expandTabRefs(windowIds: readonly string[], windows: StoreState["windows"], editorViews: StoreState["editorViews"] = {}): TabRef[] {
    return windowIds.flatMap((id): TabRef[] => {
        const win = windows[id];
        if (!win) return [];
        const documents = documentsOf(win, editorViews);
        if (documents) return documents.ids.map((doc): TabRef => ({ id, doc }));
        return roleHasTab(win.role) ? [{ id }] : [];
    });
}

/** The session's tabs as one ordered list. */
export function selectTabRefs(state: StoreState, sessionId: string): TabRef[] {
    return expandTabRefs(state.windowsBySession[sessionId] ?? EMPTY_IDS, state.windows, state.editorViews);
}

/** The windows a swipe walks through: only the ones the strip has a tab for. */
export function selectSwipeOrder(state: StoreState, sessionId: string): string[] {
    return (state.windowsBySession[sessionId] ?? EMPTY_IDS).filter((id) => expandTabRefs([id], state.windows, state.editorViews).length > 0);
}

/**
 * Which tab of `session` is live.
 *
 * The view maps resolve a window to the document it is showing, since the
 * strip holds those documents rather than the window itself.
 */
export function activeTabRef(
    session: Pick<Session, "activeWindowId">,
    windows: StoreState["windows"] = {},
    editorViews: StoreState["editorViews"] = {},
): TabRef | null {
    if (!session.activeWindowId) return null;
    const win = windows[session.activeWindowId];
    const documents = win ? documentsOf(win, editorViews) : null;
    // A window showing nothing stays a window ref: it has no document tab to
    // point at, but its layer still has to render the empty state.
    if (documents?.activeId) return { id: session.activeWindowId, doc: documents.activeId };
    return { id: session.activeWindowId };
}

/** A strip entry's identity. Window ids carry no colon, so the two parts cannot blur. */
export const tabRefKey = (ref: TabRef): string => (ref.doc === undefined ? ref.id : `${ref.id}:${ref.doc}`);

/**
 * Whether one workspace tab may be dropped beside another. A window's tab can
 * go anywhere between windows but not into the middle of another window's
 * documents, since those always sit together; a document tab stays among the
 * documents of its own window.
 */
export function workspaceTabDropAllowed(refs: readonly TabRef[], source: TabRef, target: TabRef, placement: "before" | "after"): boolean {
    if (source.doc !== undefined) return target.id === source.id && target.doc !== undefined;
    if (target.id === source.id) return false;
    if (target.doc === undefined) return true;
    const group = refs.filter((ref) => ref.id === target.id);
    const edge = placement === "before" ? group[0] : group[group.length - 1];
    return edge?.doc === target.doc;
}

/**
 * Which ordered list of tabs a cycle acts on.
 *
 * `workspace` is the session's own strip; `agents` and `terminals` are the
 * windows of one role within it; the rest are the inner lists a pane owns.
 * Naming the list rather than the caller is what lets one cycle serve the
 * keyboard, the strip and anything else that walks tabs.
 */
export type TabSource =
    | { kind: "workspace"; sessionId: string }
    | { kind: "agents"; sessionId: string }
    | { kind: "terminals"; sessionId: string }
    | { kind: "documents"; paneId: string };

export interface StripOrder {
    ids: readonly string[];
    activeId: string | null;
}

/**
 * One tab list as ids plus which is active.
 *
 * Deliberately free of labels, icons and menus: everything that walks tabs
 * needs the order and the active one, and nothing else. Being a plain function
 * of state rather than a hook is what lets commands use it outside React.
 */
export function stripOrder(state: StoreState, source: TabSource): StripOrder {
    switch (source.kind) {
        case "workspace": {
            const session = state.sessions[source.sessionId];
            const active = session ? activeTabRef(session, state.windows, state.editorViews) : null;
            return {
                ids: selectTabRefs(state, source.sessionId).map(tabRefKey),
                activeId: active ? tabRefKey(active) : null,
            };
        }
        case "agents": {
            const ids = (state.windowsBySession[source.sessionId] ?? EMPTY_IDS).filter((id) => state.windows[id]?.role === "agent");
            const activeWindowId = state.sessions[source.sessionId]?.activeWindowId ?? null;
            return { ids, activeId: activeWindowId && ids.includes(activeWindowId) ? activeWindowId : null };
        }
        case "terminals": {
            const ids = (state.windowsBySession[source.sessionId] ?? EMPTY_IDS).filter((id) => state.windows[id]?.role === "term");
            const activeWindowId = state.sessions[source.sessionId]?.activeWindowId ?? null;
            return { ids, activeId: activeWindowId && ids.includes(activeWindowId) ? activeWindowId : null };
        }
        case "documents": {
            const view = state.editorViews[source.paneId];
            return { ids: view?.openTabs ?? EMPTY_IDS, activeId: view?.activePath ?? null };
        }
    }
}

/**
 * The id `delta` steps from the active one, wrapping at both ends.
 *
 * An empty list has nothing to move to. A list whose active id is missing
 * starts from the first entry, so a cycle still goes somewhere sensible.
 */
export function nextInCycle(order: StripOrder, delta: number): string | null {
    const { ids, activeId } = order;
    if (ids.length === 0) return null;
    const index = activeId ? ids.indexOf(activeId) : -1;
    const base = index < 0 ? 0 : index;
    return ids[(base + delta + ids.length) % ids.length] ?? null;
}

export const selectActiveWindow = (state: StoreState): Window | undefined => {
    const session = selectActiveSession(state);
    return session ? state.windows[session.activeWindowId] : undefined;
};

export type WorkbenchItemState =
    StoreState["editorViews"][string] | StoreState["gitViews"][string] | StoreState["globalSearchBySession"][string] | undefined;

/** Migration adapter until every item owns its runtime state in a controller. */
export function selectItemState(state: StoreState, kind: PaneKind, itemId: string, sessionId?: string): WorkbenchItemState {
    switch (kind) {
        case "editor":
            return state.editorViews[itemId];
        case "git":
            return state.gitViews[itemId];
        case "search":
            return sessionId ? state.globalSearchBySession[sessionId] : undefined;
        case "terminal":
        case "agent":
            return undefined;
        default:
            return undefined;
    }
}

const EMPTY_IDS: readonly string[] = Object.freeze([]);
