import type { PaneKind, Session, Window, WorkspaceTabRef } from "./types";
import type { StoreState } from "./store";

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
export const selectAgentIds =
    (sessionId: string) =>
    (state: StoreState): readonly string[] =>
        state.agentsBySession[sessionId] ?? EMPTY_IDS;

/**
 * Roles the workspace rail drives, which therefore have no tab of their own.
 *
 * The rail is how you reach these and the stage is where they render, so a tab
 * for them was a second handle on one surface: "Changes" in the rail and "Diff"
 * in the strip both meant the same diff. Every other role keeps its tab, since
 * nothing else offers a way back to it.
 *
 * `files` is absent here because an editor is not one surface: the rail browses
 * the tree, but each open document is its own thing to switch between, so an
 * editor contributes a tab per document instead of none.
 */
const RAIL_DRIVEN_ROLES: ReadonlySet<string> = new Set(["diff", "search"]);

/** Whether `role` contributes a window entry to the session tab strip. */
export function roleHasTab(role: string): boolean {
    return !RAIL_DRIVEN_ROLES.has(role);
}

/**
 * Expand one session's windows and agents into strip entries.
 *
 * Rail-driven roles contribute nothing: the rail reaches them and the stage
 * renders them, so a tab would be a second handle on one surface. An editor
 * contributes one entry per open document, which is what puts its files in this
 * strip rather than a second bar inside the pane; with nothing open it
 * contributes nothing, because an empty editor is not worth a tab. Everything
 * else gets exactly one entry, and the list is derived rather than stored, so a
 * window or agent can never exist without its tab.
 */
export function expandTabRefs(
    windowIds: readonly string[],
    agentIds: readonly string[],
    windows: StoreState["windows"],
    agents: StoreState["agents"],
    editorViews: StoreState["editorViews"] = {},
): WorkspaceTabRef[] {
    return [
        ...windowIds.flatMap((id): WorkspaceTabRef[] => {
            const win = windows[id];
            if (!win) return [];
            if (win.role === "files") {
                const openTabs = editorViews[win.activePaneId]?.openTabs ?? EMPTY_IDS;
                return openTabs.map((path): WorkspaceTabRef => ({ kind: "file", id, path }));
            }
            return roleHasTab(win.role) ? [{ kind: "window", id }] : [];
        }),
        ...agentIds.filter((id) => agents[id]).map((id): WorkspaceTabRef => ({ kind: "agent", id })),
    ];
}

/**
 * The session's tabs as one ordered list: its windows, then its agents.
 */
export function selectTabRefs(state: StoreState, sessionId: string): WorkspaceTabRef[] {
    return expandTabRefs(
        state.windowsBySession[sessionId] ?? EMPTY_IDS,
        state.agentsBySession[sessionId] ?? EMPTY_IDS,
        state.windows,
        state.agents,
        state.editorViews,
    );
}

/**
 * Which tab of `session` is live. `view` is the discriminator, not a mode.
 *
 * `editorViews` resolves an editor to the document it is showing, since the
 * strip holds its documents rather than the window itself.
 */
export function activeTabRef(session: Session, windows?: StoreState["windows"], editorViews?: StoreState["editorViews"]): WorkspaceTabRef | null {
    if (session.kind === "project" && session.view === "agent") {
        return session.activeAgentId ? { kind: "agent", id: session.activeAgentId } : null;
    }
    if (!session.activeWindowId) return null;
    const win = windows?.[session.activeWindowId];
    const activePath = win?.role === "files" ? editorViews?.[win.activePaneId]?.activePath : undefined;
    // An editor showing nothing stays a window ref: it has no document tab to
    // point at, but its layer still has to render the empty state.
    if (win?.role === "files" && activePath) return { kind: "file", id: win.id, path: activePath };
    return { kind: "window", id: session.activeWindowId };
}

/**
 * The window a strip entry lives in, or null for an agent.
 *
 * Layer visibility asks this rather than matching on `kind`, so a document tab
 * shows the editor holding it and the strip and the stage cannot disagree about
 * which surface is live.
 */
export function tabRefWindowId(ref: WorkspaceTabRef | null): string | null {
    if (!ref) return null;
    return ref.kind === "agent" ? null : ref.id;
}

export const tabRefKey = (ref: WorkspaceTabRef): string => (ref.kind === "file" ? `file:${ref.id}:${ref.path}` : `${ref.kind}:${ref.id}`);

export const selectActiveWindow = (state: StoreState): Window | undefined => {
    const session = selectActiveSession(state);
    return session ? state.windows[session.activeWindowId] : undefined;
};

export type WorkbenchItemState =
    | StoreState["editorViews"][string]
    | StoreState["gitViews"][string]
    | StoreState["rundeckViews"][string]
    | StoreState["brunoViews"][string]
    | StoreState["globalSearchBySession"][string]
    | undefined;

/** Migration adapter until every item owns its runtime state in a controller. */
export function selectItemState(state: StoreState, kind: PaneKind, itemId: string, sessionId?: string): WorkbenchItemState {
    switch (kind) {
        case "editor":
            return state.editorViews[itemId];
        case "git":
            return state.gitViews[itemId];
        case "rundeck":
            return state.rundeckViews[itemId];
        case "bruno":
            return state.brunoViews[itemId];
        case "search":
            return sessionId ? state.globalSearchBySession[sessionId] : undefined;
        case "terminal":
        case "aws":
            return undefined;
    }
}

const EMPTY_IDS: readonly string[] = Object.freeze([]);
