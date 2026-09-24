import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { invokeCommand } from "../api/invoke";
import { subscribe } from "../state/bus";
import * as cmd from "../state/commands";
import { getState, setState, useStore, type StoreState } from "../state/store";
import type { PluginKind } from "../plugins/kinds";

export { fsapi as files, type DirEntry } from "../api/fs";
export { git } from "../api/git";
export { basename, dirname, joinPath, relativePath } from "../lib/paths";
export { copyText } from "../lib/clipboard";
export { confirmDialog, promptDialog } from "../state/dialog";
export { gitOverviewR } from "../state/resources.defs";
export { usePluginOverlay } from "../plugins/overlays";
export { notify, reportError, swallow } from "../state/toast";

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

/** The folder of the project in front of the person, or null when a project is not what they are looking at. */
export function useActiveProjectCwd(): string | null {
    return useStore((s) => {
        const session = s.sessions[s.activeSessionId];
        return session?.kind === "project" && session.cwd ? session.cwd : null;
    });
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
export async function pickFolder(title: string): Promise<string | null> {
    const picked = await openDialog({ directory: true, multiple: false, title });
    return typeof picked === "string" ? picked : null;
}

/** Brings this surface's session forward, opening it if needed, and returns the pane it shows in. */
export function openSurface(kind: PluginKind): string | null {
    cmd.openPluginSession(kind);
    return surfacePane(getState(), kind, false);
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
