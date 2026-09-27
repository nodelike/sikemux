import { BLANK_URL, type BrowserSnapshot, type BrowserTab } from "../api/browser";
import { DESK_PERSISTENCE_LIMITS } from "../workbench/registry";
import type { Desk, DeskTerminal, DeskView } from "./types";
import { getState, setState, type StoreState } from "./store";

export const EMPTY_DESK: Desk = { order: [], active: null, terminals: [], reveal: null };
export const EMPTY_STRIP: BrowserSnapshot = { tabs: [], activeTabId: null };
const NO_FILES: readonly string[] = [];

/** The editor view that holds a desk's files. It is keyed by agent so the files outlive the pane. */
export const deskEditorId = (agentId: string): string => `desk:${agentId}`;

export const BROWSER_ACTIVE = "browser";
export const browserKey = (tabId: string): string => `browser:${tabId}`;
export const fileKey = (path: string): string => `file:${path}`;
export const terminalKey = (id: string): string => `terminal:${id}`;

export type DeskItem =
    | { key: string; kind: "browser"; tab: BrowserTab }
    | { key: string; kind: "file"; path: string }
    | { key: string; kind: "terminal"; terminal: DeskTerminal };

/** Everything on a desk, in the order it arrived. */
export function deskItems(desk: Desk, strip: BrowserSnapshot, files: readonly string[]): DeskItem[] {
    const items = new Map<string, DeskItem>();
    for (const tab of strip.tabs) items.set(browserKey(tab.id), { key: browserKey(tab.id), kind: "browser", tab });
    for (const path of files) items.set(fileKey(path), { key: fileKey(path), kind: "file", path });
    for (const terminal of desk.terminals) items.set(terminalKey(terminal.id), { key: terminalKey(terminal.id), kind: "terminal", terminal });
    const ordered = desk.order.filter((key) => items.has(key));
    const placed = new Set(ordered);
    for (const key of items.keys()) if (!placed.has(key)) ordered.push(key);
    return ordered.map((key) => items.get(key)!);
}

export function deskItemsOf(state: Pick<StoreState, "desks" | "browserStrips" | "editorViews">, agentId: string): DeskItem[] {
    return deskItems(
        state.desks[agentId] ?? EMPTY_DESK,
        state.browserStrips[agentId] ?? EMPTY_STRIP,
        state.editorViews[deskEditorId(agentId)]?.openTabs ?? NO_FILES,
    );
}

/** Which of `BROWSER_ACTIVE`, a file key or a terminal key is on screen, falling back to the first item. */
export function shownDeskItem(desk: Desk, items: readonly DeskItem[]): string | null {
    if (desk.active === BROWSER_ACTIVE && items.some((item) => item.kind === "browser")) return BROWSER_ACTIVE;
    if (desk.active?.startsWith("file:") && desk.reveal && fileKey(desk.reveal.path) === desk.active) return desk.active;
    if (desk.active && items.some((item) => item.key === desk.active)) return desk.active;
    const first = items[0];
    if (!first) return null;
    return first.kind === "browser" ? BROWSER_ACTIVE : first.key;
}

export function isShown(item: DeskItem, shown: string | null, strip: BrowserSnapshot): boolean {
    if (item.kind === "browser") return shown === BROWSER_ACTIVE && item.tab.id === strip.activeTabId;
    return item.key === shown;
}

/**
 * What to save for a desk pane, or null when there is nothing to come back to.
 * A desk restored but never opened still holds the pages it was going to open,
 * and those are what carry across a second restart.
 */
export function deskView(paneId: string): DeskView | null {
    const state = getState();
    const agentId = state.deskPanes[paneId];
    if (!agentId) return null;
    const files = (state.editorViews[deskEditorId(agentId)]?.openTabs ?? NO_FILES).slice(0, DESK_PERSISTENCE_LIMITS.maxFiles);
    const pending = state.deskRestores[paneId];
    const strip = state.browserStrips[agentId] ?? EMPTY_STRIP;
    const saved = strip.tabs.filter((tab) => tab.url && tab.url !== BLANK_URL).slice(0, DESK_PERSISTENCE_LIMITS.maxTabs);
    const tabs = pending ? pending.tabs : saved.map((tab) => ({ url: tab.url, title: tab.title.slice(0, DESK_PERSISTENCE_LIMITS.maxTitleLength) }));
    if (tabs.length === 0 && files.length === 0) return null;
    const active = pending ? pending.activeIndex : saved.findIndex((tab) => tab.id === strip.activeTabId);
    return { agentId, tabs, activeIndex: active < 0 ? 0 : active, files: [...files] };
}

/**
 * The pages a restored desk owes the browser, handed over once. Clearing them
 * as they are taken is what keeps two renders from opening them twice.
 */
export function takeDeskRestore(paneId: string): DeskView | null {
    const pending = getState().deskRestores[paneId];
    if (!pending) return null;
    setState((s) => {
        const deskRestores = { ...s.deskRestores };
        delete deskRestores[paneId];
        return { deskRestores };
    });
    return pending;
}
