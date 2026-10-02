import { emit } from "../bus";
import { getState, mutate } from "../store";
import { DEFAULT_GLOBAL_SEARCH_VIEW } from "../types";
import { ensureRoleWindow } from "./shared";

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
