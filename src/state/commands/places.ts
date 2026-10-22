import { collectPanes } from "../layout";
import { documentsOf } from "../selectors";
import { getState, useStore, type StoreState } from "../store";
import { focusPane } from "./panes";
import { selectSession } from "./sessions";
import { selectTab } from "./tabs";

/** Somewhere the person was: a project, the tab in front, the pane in it, and the file or page that tab showed. */
export interface Place {
    sessionId: string;
    windowId: string;
    paneId: string;
    doc: string | null;
}

export interface PlaceHistory {
    canGoBack: boolean;
    canGoForward: boolean;
}

const CAPACITY = 64;

let behind: Place[] = [];
let ahead: Place[] = [];
let here: Place | null = null;
let moving = false;
let snapshot: PlaceHistory = { canGoBack: false, canGoForward: false };
const listeners = new Set<() => void>();

const samePlace = (left: Place, right: Place): boolean =>
    left.sessionId === right.sessionId && left.windowId === right.windowId && left.paneId === right.paneId && left.doc === right.doc;

export function currentPlace(state: StoreState): Place | null {
    const session = state.sessions[state.activeSessionId];
    const win = session ? state.windows[session.activeWindowId] : undefined;
    if (!session || !win) return null;
    return { sessionId: session.id, windowId: win.id, paneId: win.activePaneId, doc: documentsOf(win, state.editorViews)?.activeId ?? null };
}

/** Whether the place is still open, so going back never lands on a closed tab, pane or file. */
function stillOpen(state: StoreState, place: Place): boolean {
    const win = state.windows[place.windowId];
    if (!win || !state.sessions[place.sessionId] || !state.windowsBySession[place.sessionId]?.includes(place.windowId)) return false;
    if (!collectPanes(win.root).some((pane) => pane.id === place.paneId)) return false;
    return place.doc === null || (documentsOf(win, state.editorViews)?.ids.includes(place.doc) ?? false);
}

function changed(): void {
    const next = { canGoBack: behind.length > 0, canGoForward: ahead.length > 0 };
    if (next.canGoBack === snapshot.canGoBack && next.canGoForward === snapshot.canGoForward) return;
    snapshot = next;
    for (const listener of listeners) listener();
}

/** Forgets places since closed, and any place left beside an equal one by that. */
function prune(state: StoreState, places: Place[]): Place[] {
    return places
        .filter((place) => stillOpen(state, place))
        .filter((place, index, open) => index === 0 || !samePlace(open[index - 1] as Place, place));
}

function record(place: Place | null): void {
    if (moving || !place || (here && samePlace(here, place))) return;
    if (here) behind.push(here);
    if (behind.length > CAPACITY) behind.shift();
    here = place;
    ahead = [];
    changed();
}

function show(place: Place): void {
    if (getState().activeSessionId !== place.sessionId) selectSession(place.sessionId);
    selectTab(place.doc === null ? { id: place.windowId } : { id: place.windowId, doc: place.doc });
    focusPane(place.paneId);
}

function go(from: Place[], to: Place[]): void {
    const state = getState();
    let target = from.pop();
    while (target && !stillOpen(state, target)) target = from.pop();
    if (target) {
        if (here) to.push(here);
        here = target;
        moving = true;
        try {
            show(target);
        } finally {
            moving = false;
        }
        here = currentPlace(getState()) ?? target;
    }
    changed();
}

/** Back to where the person was before, as a browser's back button goes. */
export function goBack(): void {
    go(behind, ahead);
}

/** Forward again to where going back left. */
export function goForward(): void {
    go(ahead, behind);
}

export function placeHistory(): PlaceHistory {
    return snapshot;
}

export function subscribePlaceHistory(listener: () => void): () => void {
    listeners.add(listener);
    return () => listeners.delete(listener);
}

/** Records every place the person moves to, from now until the returned stop is called. */
export function watchPlaces(): () => void {
    behind = [];
    ahead = [];
    here = null;
    record(currentPlace(getState()));
    changed();
    return useStore.subscribe((state, previous) => {
        if (
            state.activeSessionId === previous.activeSessionId &&
            state.sessions === previous.sessions &&
            state.windows === previous.windows &&
            state.editorViews === previous.editorViews
        ) {
            return;
        }
        behind = prune(state, behind);
        ahead = prune(state, ahead);
        if (here && !stillOpen(state, here)) here = null;
        record(currentPlace(state));
        if (here && behind.length > 0 && samePlace(behind[behind.length - 1] as Place, here)) behind.pop();
        changed();
    });
}
