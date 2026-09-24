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

/** Where the page area sits, in the window's CSS pixels. */
export interface BrowserBounds {
    x: number;
    y: number;
    width: number;
    height: number;
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

export const browserApi = {
    snapshot: (agentId: string, signal?: AbortSignal) => invoke<BrowserSnapshot>("browser_snapshot", { agentId }, signal ? { signal } : undefined),
    newTab: (agentId: string, url?: string) => invoke<string>("browser_new_tab", { agentId, url: url ?? null }),
    closeAgent: (agentId: string) => invoke<void>("browser_close_agent", { agentId }),
    switchTab: (agentId: string, tabId: string) => invoke<void>("browser_switch_tab", { agentId, tabId }),
    closeTab: (agentId: string, tabId: string) => invoke<void>("browser_close_tab", { agentId, tabId }),
    navigate: (agentId: string, url: string) => invoke<void>("browser_navigate", { agentId, url }),
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
