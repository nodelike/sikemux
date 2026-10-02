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
    terminalKey,
    type DeskItem,
} from "../desks";
import { reportError } from "../toast";
import { activeAgentId, shownDeskPaneId } from "../selectors";
import { collectPanes, makePane, newId, removePane, splitPane } from "../layout";
import type { Desk } from "../types";
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
    if (openPaneId) {
        closeDesk(openPaneId);
        return;
    }
    openDesk(agentId);
    if (getState().desks[agentId]?.terminals.length || getState().editorViews[deskEditorId(agentId)]?.openTabs.length) return;
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
    mutate((d) => {
        const existing = Object.entries(d.deskPanes).find(([, owner]) => owner === agentId);
        const windowId = Object.keys(d.windows).find((id) => collectPanes(d.windows[id].root).some((pane) => pane.id === agentId));
        if (!windowId) return;
        ensureDesk(d, agentId);
        const win = d.windows[windowId];
        if (existing && collectPanes(win.root).some((pane) => pane.id === existing[0])) {
            if (focus) win.activePaneId = existing[0];
            return;
        }
        const agentPane = collectPanes(win.root).find((candidate) => candidate.id === agentId);
        const pane = makePane(agentPane?.cwd ?? "", { kind: "desk" });
        win.root = splitPane(win.root, agentId, "row", pane);
        if (focus) win.activePaneId = pane.id;
        d.deskPanes[pane.id] = agentId;
        d.zoomedPaneId = null;
    });
}

/** Hides the desk. Its files are only held by the editor on screen, so unsaved ones are asked about first. */
export function closeDesk(paneId: string): void {
    const st = getState();
    guardDiscardDirty(dirtyPathsForPane(st, paneId), "hide desk", () => removeDeskPane(paneId));
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
    mutate((d) => {
        d.deskAddressOpen = agentId;
    });
    return true;
}

export function closeDeskAddress(): void {
    mutate((d) => {
        d.deskAddressOpen = null;
    });
}
