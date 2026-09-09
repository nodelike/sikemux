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
 * The session's tabs as one ordered list: its windows, then its agents. Derived
 * rather than stored, so a window or agent can never exist without a tab.
 */
export function selectTabRefs(state: StoreState, sessionId: string): WorkspaceTabRef[] {
    const windowIds = state.windowsBySession[sessionId] ?? EMPTY_IDS;
    const agentIds = state.agentsBySession[sessionId] ?? EMPTY_IDS;
    return [
        ...windowIds.filter((id) => state.windows[id]).map((id): WorkspaceTabRef => ({ kind: "window", id })),
        ...agentIds.filter((id) => state.agents[id]).map((id): WorkspaceTabRef => ({ kind: "agent", id })),
    ];
}

/** Which tab of `session` is live. `view` is the discriminator, not a mode. */
export function activeTabRef(session: Session): WorkspaceTabRef | null {
    if (session.kind === "project" && session.view === "agent") {
        return session.activeAgentId ? { kind: "agent", id: session.activeAgentId } : null;
    }
    return session.activeWindowId ? { kind: "window", id: session.activeWindowId } : null;
}

export const tabRefKey = (ref: WorkspaceTabRef): string => `${ref.kind}:${ref.id}`;

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
