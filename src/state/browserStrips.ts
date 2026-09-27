import { useEffect } from "react";
import { browserApi, type BrowserSnapshot } from "../api/browser";
import { BROWSER_ACTIVE, browserKey, EMPTY_DESK } from "./desks";
import type { Draft } from "immer";
import { getState, mutate, type StoreState } from "./store";

/*
 * How long a lost tab report can go unnoticed. Tabs push their own changes, so
 * this is only a net under the subscription.
 */
const POLL_MS = 15_000;

/* A loading page reports itself several times over, so a burst collapses into
   the read already in flight plus one after it. */
const rereadWanted = new Map<string, boolean>();

export async function refreshBrowserStrip(agentId: string): Promise<void> {
    if (rereadWanted.has(agentId)) {
        rereadWanted.set(agentId, true);
        return;
    }
    rereadWanted.set(agentId, false);
    try {
        do {
            rereadWanted.set(agentId, false);
            const strip = await browserApi.snapshot(agentId);
            mutate((d) => placeBrowserStrip(d, agentId, strip));
        } while (rereadWanted.get(agentId));
    } finally {
        rereadWanted.delete(agentId);
    }
}

/**
 * New pages join the end of the desk, and a page the agent moved to comes
 * forward, so the desk follows the agent the way its browser does.
 */
function placeBrowserStrip(d: Draft<StoreState>, agentId: string, strip: BrowserSnapshot): void {
    const previous = d.browserStrips[agentId];
    d.browserStrips[agentId] = strip;
    const desk = (d.desks[agentId] ??= structuredClone(EMPTY_DESK));
    const live = new Set(strip.tabs.map((tab) => browserKey(tab.id)));
    desk.order = desk.order.filter((key) => !key.startsWith("browser:") || live.has(key));
    for (const key of live) if (!desk.order.includes(key)) desk.order.push(key);
    const moved = previous ? strip.activeTabId !== previous.activeTabId : desk.active === null;
    if (strip.activeTabId && moved) desk.active = BROWSER_ACTIVE;
}

/** Whose browsers are on screen, and so worth keeping a strip for. */
function browsingAgentIds(): string[] {
    return [...new Set(Object.values(getState().deskPanes))];
}

/**
 * The app's one reader of the tab strips.
 *
 * A pane draws from these rather than asking for itself, so an agent browsing
 * in a pane nobody is looking at is still the strip that gets saved.
 */
export function useBrowserStrips(): void {
    useEffect(() => {
        const controller = new AbortController();
        const syncAll = () => {
            for (const agentId of browsingAgentIds()) void refreshBrowserStrip(agentId).catch(() => {});
        };
        void browserApi.subscribeTabs(syncAll, controller.signal).catch(() => {});
        const timer = window.setInterval(syncAll, POLL_MS);
        syncAll();
        return () => {
            controller.abort();
            window.clearInterval(timer);
        };
    }, []);
}
