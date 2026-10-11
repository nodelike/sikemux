import { create } from "zustand";
import { onPaneClosed, openSurface } from "../../plugin-api/host";
import { DATABASE_BROWSER } from "./kinds";
import { forgetQuery } from "./queryState";
import { closeOtherTabs, closeProfileTabs, closeTab, openTab, type DatabaseTab, type TabStrip } from "./tabs";

export interface DatabaseView extends TabStrip {
    /** The explorer's open nodes, by key. */
    expanded: string[];
    /** The form in the main area: a new connection, or the saved one with this id. */
    editing: "new" | string | null;
}

const FIRST_VIEW: DatabaseView = { tabs: [], active: null, expanded: [], editing: null };

const useViews = create<{ views: Record<string, DatabaseView> }>(() => ({ views: {} }));

export const readDatabaseView = (paneId: string): DatabaseView => useViews.getState().views[paneId] ?? FIRST_VIEW;

function forgetClosedConsoles(before: TabStrip, after: TabStrip): void {
    const kept = new Set(after.tabs.map((tab) => tab.id));
    for (const tab of before.tabs) if (tab.kind === "console" && !kept.has(tab.id)) forgetQuery(tab.id);
}

onPaneClosed((paneId) => {
    forgetClosedConsoles(readDatabaseView(paneId), FIRST_VIEW);
    useViews.setState((state) => {
        const views = { ...state.views };
        delete views[paneId];
        return { views };
    });
});

export const useDatabaseView = (paneId: string): DatabaseView => useViews((state) => state.views[paneId] ?? FIRST_VIEW);

export function updateDatabaseView(paneId: string, change: Partial<DatabaseView>): void {
    useViews.setState((state) => ({ views: { ...state.views, [paneId]: { ...(state.views[paneId] ?? FIRST_VIEW), ...change } } }));
}

function changeTabs(paneId: string, change: (strip: TabStrip) => TabStrip): void {
    const view = readDatabaseView(paneId);
    const next = change(view);
    forgetClosedConsoles(view, next);
    updateDatabaseView(paneId, { tabs: next.tabs, active: next.active, editing: null });
}

export const showTab = (paneId: string, tab: DatabaseTab) => changeTabs(paneId, (strip) => openTab(strip, tab));
export const closeDatabaseTab = (paneId: string, id: string) => changeTabs(paneId, (strip) => closeTab(strip, id));
export const closeOtherDatabaseTabs = (paneId: string, id: string) => changeTabs(paneId, (strip) => closeOtherTabs(strip, id));
export const closeConnectionTabs = (paneId: string, profile: string) => changeTabs(paneId, (strip) => closeProfileTabs(strip, profile));

export function toggleNode(paneId: string, key: string): void {
    const { expanded } = readDatabaseView(paneId);
    updateDatabaseView(paneId, { expanded: expanded.includes(key) ? expanded.filter((each) => each !== key) : [...expanded, key] });
}

export function expandNodes(paneId: string, keys: string[]): void {
    const { expanded } = readDatabaseView(paneId);
    updateDatabaseView(paneId, { expanded: [...expanded, ...keys.filter((key) => !expanded.includes(key))] });
}

export const collapseAll = (paneId: string) => updateDatabaseView(paneId, { expanded: [] });

export const openDatabase = (): string | null => openSurface(DATABASE_BROWSER);
