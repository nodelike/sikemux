import { getCurrentWebview } from "@tauri-apps/api/webview";
import { invokeCommand as invoke } from "./invoke";
import { getIpcTransport } from "./transport";

/** The page a tab shows before it has been sent anywhere. */
export const BLANK_URL = "about:blank";

export interface BrowserTab {
    id: string;
    title: string;
    url: string;
    active: boolean;
    loading: boolean;
    canGoBack: boolean;
    canGoForward: boolean;
    /** The site's icon as a data URL, once the tab has one. */
    favicon: string | null;
    /** The agent's browser tools are working in this tab right now. */
    acting: boolean;
}

export interface BrowserSnapshot {
    tabs: BrowserTab[];
    activeTabId: string | null;
}

/** A remembered page offered while someone types an address. */
export interface AddressSuggestion {
    url: string;
    title: string;
    /** The address as the bar writes it: no scheme and no `www.`. */
    address: string;
    icon: string | null;
}

export interface AddressSuggestions {
    /** A remembered address that begins with what was typed, which the bar finishes in place. */
    completion: AddressSuggestion | null;
    pages: AddressSuggestion[];
    /** Whether the typed text, sent as it is, would be a web search. */
    searches: boolean;
    searchUrl: string;
}

/** A rounded rectangle of the page that the app shows through, in the page's own CSS pixels. */
export interface BrowserHole {
    x: number;
    y: number;
    width: number;
    height: number;
    radius: number;
}

/** Where the page area sits, in the window's CSS pixels, how much of either side lies off stage, and what shows through it. */
export interface BrowserBounds {
    x: number;
    y: number;
    width: number;
    height: number;
    clipLeft: number;
    clipRight: number;
    holes: BrowserHole[];
    /** How dark a shade to lay over the page, from 0 to 1. */
    dim?: number;
}

/** A command chord pressed while a page had keyboard focus. */
export interface BrowserShortcut {
    agentId: string;
    tabId: string;
    key: string;
    code: string;
    shift: boolean;
    alt: boolean;
}

/** A file a tab handed to the download folder; announced at start and end. */
export interface BrowserDownload {
    agentId: string;
    tabId: string;
    url: string;
    path: string;
    state: "started" | "finished" | "failed";
}

/** Pages are webviews of their own, and one that has the keyboard keeps it until the app's webview takes it back. */
export function takeKeyboardFromPages(): Promise<void> {
    return getCurrentWebview().setFocus();
}

export const browserApi = {
    snapshot: (agentId: string, signal?: AbortSignal) => invoke<BrowserSnapshot>("browser_snapshot", { agentId }, signal ? { signal } : undefined),
    newTab: (agentId: string, url?: string) => invoke<string>("browser_new_tab", { agentId, url: url ?? null }),
    closeAgent: (agentId: string) => invoke<void>("browser_close_agent", { agentId }),
    switchTab: (agentId: string, tabId: string) => invoke<void>("browser_switch_tab", { agentId, tabId }),
    closeTab: (agentId: string, tabId: string) => invoke<void>("browser_close_tab", { agentId, tabId }),
    navigate: (agentId: string, url: string) => invoke<void>("browser_navigate", { agentId, url }),
    suggest: (query: string) => invoke<AddressSuggestions>("browser_suggest", { query }),
    back: (agentId: string) => invoke<void>("browser_back", { agentId }),
    forward: (agentId: string) => invoke<void>("browser_forward", { agentId }),
    reload: (agentId: string) => invoke<void>("browser_reload", { agentId }),
    /** `null` parks the agent's page off screen until the pane places it again. */
    setBounds: (agentId: string, bounds: BrowserBounds | null) => invoke<void>("browser_set_bounds", { agentId, bounds }),
    subscribeTabs: (listener: () => void, signal: AbortSignal) => getIpcTransport().subscribe("browser-tabs-changed", listener, { signal }),
    subscribeShortcuts: (listener: (shortcut: BrowserShortcut) => void, signal: AbortSignal) =>
        getIpcTransport().subscribe<BrowserShortcut>("browser-shortcut", (event) => listener(event.payload), { signal }),
    subscribeActing: (listener: (agentId: string) => void, signal: AbortSignal) =>
        getIpcTransport().subscribe<string>("browser-agent-acting", (event) => listener(event.payload), { signal }),
    subscribeDownloads: (listener: (download: BrowserDownload) => void, signal: AbortSignal) =>
        getIpcTransport().subscribe<BrowserDownload>("browser-download", (event) => listener(event.payload), { signal }),
};
