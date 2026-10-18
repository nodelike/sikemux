import { dialogsApi } from "../api/dialogs";
import { invokeCommand } from "../api/invoke";
import { subscribe } from "../state/bus";
import * as cmd from "../state/commands";
import { getState, setState, useStore, type StoreState } from "../state/store";
import { copyText } from "../lib/clipboard";
import { notify, reportError } from "../state/toast";
import type { PluginKind } from "../plugins/kinds";
import type { GitStatus } from "../api/git";
import type { GitArea } from "../state/types";
import type { BoardCard, CardTicket, NewCard, ProjectBoard } from "../agents/agentBoard";
import { agentBoardApi } from "../api/agentBoard";
import { nearestProjectSessionId } from "../agents/agentTargets";

export { fsapi as files, type DirEntry } from "../api/fs";
export { git } from "../api/git";
export { basename, dirname, joinPath, relativePath } from "../lib/paths";
export { copyText };
export { confirmDialog, promptDialog } from "../state/dialog";
export { gitOverviewR } from "../state/resources.defs";
export { usePluginOverlay } from "../plugins/overlays";
export { useModalFocus } from "../hooks/useModalFocus";
export { notify, reportError };
export { swallow } from "../state/toast";
export { postNotification as notifyDesktop } from "../agents/agentNotifications";

/** The branch checked out, or null on a detached HEAD, which git reports as a branch named `HEAD`. */
export function currentBranch(status: Pick<GitStatus, "branch"> | null | undefined): string | null {
    const branch = status?.branch.trim() ?? "";
    return branch && branch !== "HEAD" ? branch : null;
}

/** Copies `text` and says what was copied, such as "the link"; a copy that fails is reported. */
export function copyWithNotice(text: string, what: string): void {
    void copyText(text).then(() => notify("success", `copied ${what}`), reportError("copy"));
}

export function openUrl(url: string): Promise<void> {
    return invokeCommand<void>("open_url", { url, app: null, shortcut: null });
}

/** Opens a sign-in page in the browser the person chose for single sign-on in Settings. */
export function openSignInUrl(url: string): Promise<void> {
    const { cloudBrowser, cloudBrowserShortcut } = getState();
    return invokeCommand<void>("open_url", { url, app: cloudBrowser || null, shortcut: cloudBrowserShortcut || null });
}

/** Brings the single sign-on browser forward, for a CLI that opens its own sign-in page. */
export function focusSignInBrowser(): Promise<void> {
    const { cloudBrowser, cloudBrowserShortcut } = getState();
    if (!cloudBrowser) return Promise.resolve();
    return invokeCommand<void>("macos_focus_app", { app: cloudBrowser, shortcut: cloudBrowserShortcut || null });
}

function clipped(text: string, max: number, tail: string): string {
    const points = Array.from(text);
    return points.length > max ? points.slice(0, max).join("") + tail : text;
}

export interface TicketCard {
    title: string;
    details: string;
    ticket: CardTicket;
}

function fitted(card: TicketCard): NewCard {
    return {
        ...card,
        title: clipped(card.title.replace(/\s+/g, " ").trim(), 199, "…"),
        details: clipped(card.details, 4000, "\n\n[Cut short; the ticket has the rest.]"),
    };
}

function ticketCard(board: ProjectBoard, key: string): BoardCard | undefined {
    return board.cards.find((card) => card.ticket?.key.toLowerCase() === key.toLowerCase());
}

/**
 * Puts a ticket on the board of the nearest open project as a card, with its subtasks as cards under it. It waits
 * on the tickets that block it, and the tickets it blocks wait on it, when those are on the board too. Only the
 * person does this.
 */
export async function sendToBoard(card: TicketCard & { subtasks?: TicketCard[]; blockedBy?: string[]; blocks?: string[] }): Promise<boolean> {
    const state = getState();
    const session = state.sessions[nearestProjectSessionId(state) ?? ""];
    const folder = session?.cwd;
    if (!folder) {
        notify("error", "Open a project to put this on its board");
        return false;
    }
    let board: ProjectBoard;
    try {
        board = await agentBoardApi.add(folder, fitted(card));
    } catch (error) {
        reportError("send to the board")(error);
        return false;
    }
    const made = ticketCard(board, card.ticket.key);
    const then = async (label: string, work: () => Promise<ProjectBoard>) => {
        try {
            board = await work();
        } catch (error) {
            reportError(label)(error);
        }
    };
    if (made) {
        for (const sub of card.subtasks ?? []) {
            const there = ticketCard(board, sub.ticket.key);
            if (!there) await then(`put ${sub.ticket.key} under it`, () => agentBoardApi.add(folder, { ...fitted(sub), parent: made.id }));
            else if (there.parent === null)
                await then(`put ${sub.ticket.key} under it`, () => agentBoardApi.update(folder, there.id, { parent: made.id }));
        }
        const waits = (card.blockedBy ?? []).flatMap((key) => ticketCard(board, key)?.id ?? []);
        if (waits.length > 0) await then("make it wait on its blockers", () => agentBoardApi.update(folder, made.id, { dependsOn: waits }));
        for (const key of card.blocks ?? []) {
            const blocked = ticketCard(board, key);
            if (blocked && !blocked.dependsOn.includes(made.id)) {
                await then(`make ${key} wait on it`, () => agentBoardApi.update(folder, blocked.id, { dependsOn: [...blocked.dependsOn, made.id] }));
            }
        }
    }
    cmd.setAgentBoard(folder, board);
    notify("success", `put ${card.ticket.key} on ${session.name}'s board`);
    return true;
}

/** The folder of the project in front of the person, or null when a project is not what they are looking at. */
export function useActiveProjectCwd(): string | null {
    return useStore((s) => {
        const session = s.sessions[s.activeSessionId];
        return session?.kind === "project" && session.cwd ? session.cwd : null;
    });
}

/** The project in front, or else the one the person last had in front, or else the first one open. */
export function useNearestProject(): { name: string; cwd: string } | null {
    const session = useStore((s) => s.sessions[nearestProjectSessionId(s) ?? ""]);
    return session?.cwd ? { name: session.name, cwd: session.cwd } : null;
}

function surfacePane(state: StoreState, kind: PluginKind, activeOnly: boolean): string | null {
    const session = activeOnly ? state.sessions[state.activeSessionId] : Object.values(state.sessions).find((s) => s.kind === kind);
    if (session?.kind !== kind) return null;
    return state.windows[session.activeWindowId]?.activePaneId ?? null;
}

/** The pane showing this surface, when its session is the one in front. */
export function useActiveSurfacePane(kind: PluginKind): string | null {
    return useStore((s) => surfacePane(s, kind, true));
}

/** The same, read once, for a shortcut or command deciding whether it applies. */
export function activeSurfacePane(kind: PluginKind): string | null {
    return surfacePane(getState(), kind, true);
}

/** Asks the person for a folder; null when they cancel. */
export function pickFolder(title: string): Promise<string | null> {
    return dialogsApi.pickFolder({ title });
}

/** Asks the person for a file, offering only these extensions when any are given; null when they cancel. */
export function pickFile(title: string, filter?: { name: string; extensions: string[] }): Promise<string | null> {
    return dialogsApi.pickFile({ title, filters: filter ? [filter] : undefined });
}

/** Runs a program in a terminal over the window, settling once the person closes it. */
export function runInTerminal(title: string, argv: string[]): Promise<void> {
    const command = argv.map(cmd.shellQuote).join(" ");
    cmd.runCustomCommand({ id: `plugin-terminal:${title}`, title, detail: "", command, contexts: [], placement: "popup" });
    const opened = getState().commandPopup;
    if (!opened) return Promise.resolve();
    return new Promise((resolve) => {
        const stop = useStore.subscribe((state) => {
            if (state.commandPopup?.id === opened.id) return;
            stop();
            resolve();
        });
    });
}

/** Brings this surface's session forward, opening it if needed, and returns the pane it shows in. */
export function openSurface(kind: PluginKind): string | null {
    cmd.openPluginSession(kind);
    return surfacePane(getState(), kind, false);
}

/** Opens the git pane at its local workbench or at one of the code host's sections. */
export function openGitArea(area: GitArea): string | null {
    return cmd.openGitArea(area);
}

export function onPaneClosed(listener: (paneId: string) => void): () => void {
    return subscribe("pane-closed", (event) => listener(event.paneId));
}

/** A plugin's palette and the app's own palettes are never open together. */
export function closeCorePalettes(): void {
    setState({ pickerOpen: false, filePaletteOpen: false, agentPaletteOpen: false });
}

export function useCorePaletteOpen(): boolean {
    return useStore((s) => s.pickerOpen || s.filePaletteOpen || s.agentPaletteOpen);
}
