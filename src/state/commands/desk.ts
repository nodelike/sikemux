import { browserApi } from "../../api/browser";
import { taskPtyBindings, type TaskTerminalPresentationRequest } from "../../tasks/nativeRuntime";
import { emit } from "../bus";
import { getState, mutate, type StoreState } from "../store";
import type { Draft } from "immer";
import { refreshBrowserStrip } from "../browserStrips";
import {
    BROWSER_ACTIVE,
    deskEditorId,
    deskItemsOf,
    EMPTY_DESK,
    EMPTY_STRIP,
    fileKey,
    isShown,
    shownDeskItem,
    simulatorKey,
    terminalKey,
    type DeskItem,
} from "../desks";
import { reportError } from "../toast";
import { announceDeskMotion, DESK_MOTION_MS, type DeskHeading } from "../deskMotion";
import { holdStageMotion } from "../nativeViews";
import { canAnimate } from "../../lib/motion";
import { activeAgentId, shownDeskPaneId } from "../selectors";
import { collectPanes, computeLayout, findSplit, makePane, newId, removePane, setSplitSizes, splitPane } from "../layout";
import type { Desk, LayoutNode, SplitNode } from "../types";
import { setEditorView } from "./editor";
import { dirtyPathsForPane, dropDeskPaneState, guardDiscardDirty } from "./shared";

function activeBrowserAgentId(): string | null {
    const st = getState();
    const session = st.sessions[st.activeSessionId];
    if (session?.kind !== "project") return null;
    return activeAgentId(st, session);
}

function ensureDesk(d: Draft<StoreState>, agentId: string): Desk {
    return (d.desks[agentId] ??= structuredClone(EMPTY_DESK));
}

function setDeskActive(agentId: string, active: string): void {
    mutate((d) => {
        ensureDesk(d, agentId).active = active;
    });
}

export function newBrowserTab(forAgentId?: string): boolean {
    const agentId = forAgentId ?? activeBrowserAgentId();
    if (!agentId) return false;
    openDesk(agentId);
    setDeskActive(agentId, BROWSER_ACTIVE);
    void browserApi.newTab(agentId).catch(reportError("open browser tab"));
    openDeskAddress(agentId);
    return true;
}

export function openUrlOnDesk(agentId: string, url: string): void {
    openDesk(agentId);
    setDeskActive(agentId, BROWSER_ACTIVE);
    void browserApi.newTab(agentId, url).catch(reportError("open link in browser"));
}

/** Hiding the desk keeps what is on it, so showing it again brings it back as it was. */
export function toggleDesk(agentId: string): void {
    const openPaneId = shownDeskPaneId(getState(), agentId);
    if (openPaneId && travels.get(openPaneId)?.heading === "closed") {
        travelDesk(openPaneId, "open");
        return;
    }
    if (openPaneId) {
        closeDesk(openPaneId);
        return;
    }
    openDesk(agentId);
    const desk = getState().desks[agentId];
    if (desk?.terminals.length || desk?.simulators.length || getState().editorViews[deskEditorId(agentId)]?.openTabs.length) return;
    void browserApi
        .snapshot(agentId)
        .then((snapshot) => (snapshot.tabs.length === 0 ? browserApi.newTab(agentId) : undefined))
        .catch(reportError("open browser tab"));
}

/** Brings the desk on screen for an agent that put something on it, leaving focus where the person had it. */
export function revealDesk(agentId: string): void {
    if (shownDeskPaneId(getState(), agentId)) return;
    openDesk(agentId, { focus: false });
}

/** The agent started working in its browser, so the page comes forward on its desk. */
export function showDeskBrowser(agentId: string): void {
    revealDesk(agentId);
    setDeskActive(agentId, BROWSER_ACTIVE);
}

/**
 * Put the agent's desk beside it, once.
 *
 * The pane is a leaf like any other, so it splits, resizes and closes through
 * the layout rather than through anything the desk owns itself.
 */
export function openDesk(agentId: string, opts: { focus?: boolean } = {}): void {
    const focus = opts.focus ?? true;
    let created = null as { paneId: string; windowId: string } | null;
    let reopened = null as string | null;
    mutate((d) => {
        const existing = Object.entries(d.deskPanes).find(([, owner]) => owner === agentId);
        const windowId = Object.keys(d.windows).find((id) => collectPanes(d.windows[id].root).some((pane) => pane.id === agentId));
        if (!windowId) return;
        ensureDesk(d, agentId);
        const win = d.windows[windowId];
        if (existing && collectPanes(win.root).some((pane) => pane.id === existing[0])) {
            if (focus) win.activePaneId = existing[0];
            reopened = existing[0];
            return;
        }
        const agentPane = collectPanes(win.root).find((candidate) => candidate.id === agentId);
        const pane = makePane(agentPane?.cwd ?? "", { kind: "desk" });
        win.root = splitPane(win.root, agentId, "row", pane);
        if (focus) win.activePaneId = pane.id;
        d.deskPanes[pane.id] = agentId;
        d.zoomedPaneId = null;
        created = { paneId: pane.id, windowId };
    });
    if (reopened && travels.get(reopened)?.heading === "closed") travelDesk(reopened, "open");
    if (!created || !canAnimate(document.body)) return;
    const found = deskSplit(created.windowId, created.paneId);
    if (!found) return;
    const open = found.split.sizes;
    setSizes(created.windowId, found.split.id, folded(open, found.index));
    travelDesk(created.paneId, "open", open);
}

function splitHolding(node: LayoutNode, paneId: string): SplitNode | null {
    if (node.type === "pane") return null;
    if (node.children.some((child) => child.type === "pane" && child.id === paneId)) return node;
    for (const child of node.children) {
        const found = splitHolding(child, paneId);
        if (found) return found;
    }
    return null;
}

/* Not zero: a saved layout may not hold an empty pane, and the split can be saved mid-way. */
const SLIVER = 0.01;

function deskSplit(windowId: string, paneId: string): { split: SplitNode; index: number } | null {
    const root = getState().windows[windowId]?.root;
    const split = root ? splitHolding(root, paneId) : null;
    const index = split?.children.findIndex((child) => child.type === "pane" && child.id === paneId) ?? -1;
    return split && index >= 1 ? { split, index } : null;
}

/** The desk's sizes with its share handed to the pane before it, all but a sliver. */
function folded(sizes: number[], index: number): number[] {
    const next = sizes.slice();
    const sliver = Math.min(SLIVER, next[index]);
    next[index - 1] += next[index] - sliver;
    next[index] = sliver;
    return next;
}

interface DeskTravel {
    windowId: string;
    splitId: string;
    index: number;
    open: number[];
    closed: number[];
    heading: DeskHeading;
    from: number[];
    last: number[];
    begun: number;
    ms: number;
    frame: number;
    release: () => void;
}

/* At most one movement per desk. Toggling mid-way turns it around from where it is. */
const travels = new Map<string, DeskTravel>();

function setSizes(windowId: string, splitId: string, sizes: number[]): void {
    mutate((d) => {
        d.windows[windowId].root = setSplitSizes(d.windows[windowId].root, splitId, sizes);
    });
}

function windowHolding(paneId: string): string | undefined {
    const { windows } = getState();
    return Object.keys(windows).find((id) => collectPanes(windows[id].root).some((pane) => pane.id === paneId));
}

/* The desk opens and closes the way a divider drag would: the agent gives up
   exactly the room the desk takes, so the two read as one movement. Each frame
   is a size change like a drag's, so terminals and the browser page follow it.
   Returns false when there was nothing to move, and the caller does it at once. */
function travelDesk(paneId: string, heading: DeskHeading, open?: number[]): boolean {
    let travel = travels.get(paneId);
    if (!travel) {
        const windowId = windowHolding(paneId);
        const found = windowId ? deskSplit(windowId, paneId) : null;
        if (!windowId || !found) return false;
        const sizes = open ?? found.split.sizes;
        travel = {
            windowId,
            splitId: found.split.id,
            index: found.index,
            open: sizes,
            closed: folded(sizes, found.index),
            heading,
            from: found.split.sizes,
            last: found.split.sizes,
            begun: 0,
            ms: 0,
            frame: 0,
            release: holdStageMotion(),
        };
        travels.set(paneId, travel);
    }
    const to = heading === "open" ? travel.open : travel.closed;
    const { index } = travel;
    const left = Math.abs(to[index] - travel.last[index]) / (travel.open[index] - travel.closed[index] || 1);
    travel.heading = heading;
    travel.from = travel.last;
    travel.begun = performance.now();
    travel.ms = Math.max(120, DESK_MOTION_MS * Math.min(1, left));
    const root = getState().windows[travel.windowId].root;
    announceDeskMotion(paneId, {
        kind: "moving",
        heading,
        ms: travel.ms,
        openShare: shareOf(setSplitSizes(root, travel.splitId, travel.open), paneId),
        currentShare: shareOf(root, paneId),
        appearing: false,
    });
    if (!travel.frame) travel.frame = requestAnimationFrame((now) => stepDesk(paneId, now));
    return true;
}

function stepDesk(paneId: string, now: number): void {
    const travel = travels.get(paneId);
    if (!travel) return;
    travel.frame = 0;
    const current = findSplitIn(travel.windowId, travel.splitId);
    /* A divider drag or a removed pane took the split over, so it stays where it was put. */
    if (!current || current.sizes.length !== travel.last.length || current.sizes.some((size, i) => size !== travel.last[i])) {
        settle(paneId, travel);
        return;
    }
    const to = travel.heading === "open" ? travel.open : travel.closed;
    const t = Math.min(1, Math.max(0, (now - travel.begun) / travel.ms));
    const eased = 1 - (1 - t) ** 4;
    travel.last = t === 1 ? to : to.map((size, i) => travel.from[i] + (size - travel.from[i]) * eased);
    setSizes(travel.windowId, travel.splitId, travel.last);
    if (t < 1) {
        travel.frame = requestAnimationFrame((next) => stepDesk(paneId, next));
        return;
    }
    settle(paneId, travel);
}

function settle(paneId: string, travel: DeskTravel): void {
    travels.delete(paneId);
    travel.release();
    announceDeskMotion(paneId, { kind: "settled" });
    if (travel.heading === "closed") removeDeskPane(paneId);
}

/** How much of the window's width the pane takes in this layout. */
function shareOf(root: LayoutNode, paneId: string): number {
    return computeLayout(root).panes.get(paneId)?.w ?? 0;
}

function foldDesk(paneId: string): void {
    if (!canAnimate(document.body) || !travelDesk(paneId, "closed")) removeDeskPane(paneId);
}

function findSplitIn(windowId: string, splitId: string): SplitNode | null {
    const root = getState().windows[windowId]?.root;
    return root ? findSplit(root, splitId) : null;
}

/** Hides the desk. Its files are only held by the editor on screen, so unsaved ones are asked about first. */
export function closeDesk(paneId: string): void {
    const st = getState();
    guardDiscardDirty(dirtyPathsForPane(st, paneId), "hide desk", () => foldDesk(paneId));
}

/**
 * Everything left the desk, or the pane came back from a layout without the
 * agent that gave it meaning — either way there is nothing left for it to show.
 */
export function removeDeskPane(paneId: string): void {
    mutate((d) => {
        for (const id of Object.keys(d.windows)) {
            const win = d.windows[id];
            if (!collectPanes(win.root).some((pane) => pane.id === paneId)) continue;
            const root = removePane(win.root, paneId);
            if (root === null) return;
            win.root = root;
            const remaining = collectPanes(root);
            if (!remaining.some((pane) => pane.id === win.activePaneId)) win.activePaneId = remaining[0]?.id ?? win.activePaneId;
            d.zoomedPaneId = null;
        }
        dropDeskPaneState(d, paneId);
    });
}

/** Opens a file on the agent's desk, at a line when one is given. */
export function openFileOnDesk(agentId: string, path: string, line?: number, character?: number, opts: { focus?: boolean } = {}): void {
    openDesk(agentId, { focus: opts.focus ?? true });
    mutate((d) => {
        const desk = ensureDesk(d, agentId);
        const key = fileKey(path);
        if (!desk.order.includes(key)) desk.order.push(key);
        desk.active = key;
        desk.reveal = { path, line, character, seq: (desk.reveal?.seq ?? 0) + 1 };
    });
}

export function consumeDeskReveal(agentId: string, seq: number): void {
    mutate((d) => {
        const desk = d.desks[agentId];
        if (desk?.reveal?.seq === seq) desk.reveal = null;
    });
}

/**
 * Puts a task terminal on the agent's desk and returns the id its process is
 * bound to. A rerun of the same task comes back to the terminal it had.
 */
export function openDeskTerminal(
    agentId: string,
    request: Pick<TaskTerminalPresentationRequest, "terminalKey" | "label" | "cwd" | "background">,
): string {
    let id = "";
    mutate((d) => {
        const desk = ensureDesk(d, agentId);
        let terminal = desk.terminals.find((candidate) => candidate.terminalKey === request.terminalKey);
        if (!terminal) {
            terminal = { id: newId("desk-terminal"), terminalKey: request.terminalKey, label: request.label, cwd: request.cwd };
            desk.terminals.push(terminal);
            desk.order.push(terminalKey(terminal.id));
        }
        terminal.label = request.label;
        terminal.cwd = request.cwd;
        if (!request.background || desk.active === null) desk.active = terminalKey(terminal.id);
        id = terminal.id;
    });
    if (!request.background) revealDesk(agentId);
    return id;
}

/** Shows the iOS Simulator on the agent's desk; a desk has one, which the person and the agent share. */
export function openDeskSimulator(agentId: string, opts: { focus?: boolean } = {}): string {
    let id = "";
    openDesk(agentId, { focus: opts.focus ?? true });
    mutate((d) => {
        const desk = ensureDesk(d, agentId);
        let simulator = desk.simulators[0];
        if (!simulator) {
            simulator = { id: newId("desk-simulator"), udid: null, deviceName: null };
            desk.simulators.push(simulator);
            desk.order.push(simulatorKey(simulator.id));
        }
        desk.active = simulatorKey(simulator.id);
        id = simulator.id;
    });
    return id;
}

export function setDeskSimulatorDevice(agentId: string, id: string, device: { udid: string; name: string } | null): void {
    mutate((d) => {
        const simulator = d.desks[agentId]?.simulators.find((candidate) => candidate.id === id);
        if (!simulator) return;
        simulator.udid = device?.udid ?? null;
        simulator.deviceName = device?.name ?? null;
    });
}

export function showDeskTerminal(agentId: string, id: string): void {
    revealDesk(agentId);
    setDeskActive(agentId, terminalKey(id));
}

/** The desk terminal showing a task execution, if one is. */
export function deskTerminalFor(executionId: string): { agentId: string; id: string } | null {
    for (const [agentId, desk] of Object.entries(getState().desks))
        for (const terminal of desk.terminals)
            if (taskPtyBindings.getSnapshot(terminal.id)?.executionId === executionId) return { agentId, id: terminal.id };
    return null;
}

export function selectDeskItem(agentId: string, item: DeskItem): void {
    if (item.kind === "browser") {
        setDeskActive(agentId, BROWSER_ACTIVE);
        void browserApi
            .switchTab(agentId, item.tab.id)
            .then(() => refreshBrowserStrip(agentId))
            .catch(reportError("switch browser tab"));
        return;
    }
    setDeskActive(agentId, item.key);
    if (item.kind === "file") setEditorView(deskEditorId(agentId), { activePath: item.path });
}

export function closeDeskItem(agentId: string, item: DeskItem): void {
    const items = deskItemsOf(getState(), agentId);
    const desk = getState().desks[agentId] ?? EMPTY_DESK;
    const shown = shownDeskItem(desk, items);
    const strip = getState().browserStrips[agentId];
    if (strip && isShown(item, shown, strip)) {
        const at = items.findIndex((candidate) => candidate.key === item.key);
        const next = items[at + 1] ?? items[at - 1];
        if (next) selectDeskItem(agentId, next);
    }
    if (item.kind === "browser") {
        void browserApi
            .closeTab(agentId, item.tab.id)
            .then(() => refreshBrowserStrip(agentId))
            .catch(reportError("close browser tab"));
        return;
    }
    if (item.kind === "file") {
        emit({ type: "close-file", paneId: deskEditorId(agentId), path: item.path });
        return;
    }
    if (item.kind === "simulator") {
        mutate((d) => {
            const current = d.desks[agentId];
            if (!current) return;
            current.simulators = current.simulators.filter((simulator) => simulator.id !== item.simulator.id);
            current.order = current.order.filter((key) => key !== item.key);
        });
        return;
    }
    taskPtyBindings.release(item.terminal.id);
    mutate((d) => {
        const current = d.desks[agentId];
        if (!current) return;
        current.terminals = current.terminals.filter((terminal) => terminal.id !== item.terminal.id);
        current.order = current.order.filter((key) => key !== item.key);
    });
}

/** Closes the page, file or terminal the desk is showing; false when it shows nothing. */
export function closeShownDeskTab(agentId: string): boolean {
    const state = getState();
    const items = deskItemsOf(state, agentId);
    const shown = shownDeskItem(state.desks[agentId] ?? EMPTY_DESK, items);
    const item = items.find((candidate) => isShown(candidate, shown, state.browserStrips[agentId] ?? EMPTY_STRIP));
    if (item) closeDeskItem(agentId, item);
    return !!item;
}

export function cycleDeskTab(agentId: string, delta: number): void {
    const state = getState();
    const items = deskItemsOf(state, agentId);
    if (items.length < 2) return;
    const shown = shownDeskItem(state.desks[agentId] ?? EMPTY_DESK, items);
    const current = Math.max(
        0,
        items.findIndex((item) => isShown(item, shown, state.browserStrips[agentId] ?? EMPTY_STRIP)),
    );
    selectDeskItem(agentId, items[(current + delta + items.length) % items.length]);
}

export function reloadBrowserTab(): boolean {
    const agentId = activeBrowserAgentId();
    if (!agentId) return false;
    void browserApi.reload(agentId).catch(reportError("reload browser"));
    return true;
}

export function browserHistory(delta: number): boolean {
    const agentId = activeBrowserAgentId();
    if (!agentId) return false;
    void (delta < 0 ? browserApi.back(agentId) : browserApi.forward(agentId)).catch(reportError("navigate browser history"));
    return true;
}

export function focusBrowserAddress(): boolean {
    const agentId = activeBrowserAgentId();
    if (!agentId) return false;
    const hasPage = (getState().browserStrips[agentId]?.tabs.length ?? 0) > 0;
    openDesk(agentId);
    setDeskActive(agentId, BROWSER_ACTIVE);
    if (!hasPage) void browserApi.newTab(agentId).catch(reportError("open browser address"));
    openDeskAddress(agentId);
    return true;
}

function openDeskAddress(agentId: string): void {
    mutate((d) => {
        d.deskAddressOpen = agentId;
    });
}

export function closeDeskAddress(): void {
    mutate((d) => {
        d.deskAddressOpen = null;
    });
}
