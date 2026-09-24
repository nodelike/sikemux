import { create } from "zustand";
import {
    activeSurfacePane,
    basename,
    closeCorePalettes,
    dirname,
    files,
    joinPath,
    notify,
    onPaneClosed,
    openSurface,
    pickFolder,
    reportError,
} from "../../plugin-api/host";
import { invalidate, resource } from "../../plugin-api/resources";
import { definePluginSettings } from "../../plugin-api/settings";
import { BRUNO_CLIENT, BRUNO_PLUGIN_ID } from "./kinds";
import { loadCollection } from "./lib/collection";
import { parseRequest } from "./lib/parse";
import { serializeRequest } from "./lib/serialize";
import { emptyRequest, type BruCollection } from "./lib/types";
import { brunoDrafts, forgetBrunoPane, setBrunoDraft, setBrunoSecret } from "./runtime";

export type BrunoReqTab = "params" | "body" | "headers" | "auth" | "vars" | "script" | "docs";
export type BrunoResTab = "body" | "headers" | "timeline" | "tests";

export interface BrunoView {
    /** Open request tabs, as file paths, in strip order. */
    openPaths: string[];
    activeRequestPath: string | null;
    reqTab: BrunoReqTab;
    resTab: BrunoResTab;
    /** Request pane width in the request/response split, as a percent. */
    reqPanePct: number;
    secretsOpen: boolean;
}

export const DEFAULT_BRUNO_VIEW: BrunoView = {
    openPaths: [],
    activeRequestPath: null,
    reqTab: "params",
    resTab: "body",
    reqPanePct: 50,
    secretsOpen: false,
};

export interface BrunoSettings {
    /** The workspace loaded now; empty until one is chosen. */
    collectionPath: string;
    /** The environment chosen for each collection, by its root folder. */
    selectedEnvs: Record<string, string>;
    /** Workspaces opened before, most recent first, so they stay one pick away. */
    workspaces: string[];
}

const isRecord = (value: unknown): value is Record<string, unknown> => !!value && typeof value === "object" && !Array.isArray(value);

function decodeSettings(saved: unknown): BrunoSettings {
    const raw = isRecord(saved) ? saved : {};
    const selectedEnvs: Record<string, string> = {};
    for (const [path, env] of Object.entries(isRecord(raw.selectedEnvs) ? raw.selectedEnvs : {})) {
        if (typeof env === "string" && env) selectedEnvs[path] = env;
    }
    const workspaces = Array.isArray(raw.workspaces) ? raw.workspaces.filter((path): path is string => typeof path === "string" && !!path) : [];
    return {
        collectionPath: typeof raw.collectionPath === "string" ? raw.collectionPath : "",
        selectedEnvs,
        workspaces: [...new Set(workspaces)],
    };
}

export const brunoSettings = definePluginSettings(BRUNO_PLUGIN_ID, decodeSettings);

export const brunoCollectionR = resource({
    kind: "bruno.collection",
    fetch: (rootPath: string): Promise<BruCollection> => loadCollection(rootPath),
    staleAfterMs: 5 * 60_000,
});

interface BrunoPlugin {
    views: Record<string, BrunoView>;
    /** The loaded collection, kept here too so a request's tab can show its name and method. */
    collection: BruCollection | null;
    requestPalette: boolean;
    environmentPalette: boolean;
    workspacePalette: boolean;
}

export const useBruno = create<BrunoPlugin>()(() => ({
    views: {},
    collection: null,
    requestPalette: false,
    environmentPalette: false,
    workspacePalette: false,
}));

export function viewOf(paneId: string): BrunoView {
    return useBruno.getState().views[paneId] ?? DEFAULT_BRUNO_VIEW;
}

export function useBrunoView(paneId: string): BrunoView {
    return useBruno((state) => state.views[paneId] ?? DEFAULT_BRUNO_VIEW);
}

function patchView(paneId: string, patch: (current: BrunoView) => BrunoView | void): void {
    useBruno.setState((state) => {
        const current = state.views[paneId] ?? DEFAULT_BRUNO_VIEW;
        return { views: { ...state.views, [paneId]: patch(current) ?? current } };
    });
}

onPaneClosed((paneId) => {
    forgetBrunoPane(paneId);
    if (!(paneId in useBruno.getState().views)) return;
    useBruno.setState((state) => {
        const views = { ...state.views };
        delete views[paneId];
        return { views };
    });
});

export function rememberCollection(collection: BruCollection | null): void {
    if (useBruno.getState().collection !== collection) useBruno.setState({ collection });
}

/** Remember a workspace so it stays reopenable, most recent first. */
export function registerBrunoWorkspace(path: string): void {
    brunoSettings.update((settings) => ({ ...settings, workspaces: [path, ...settings.workspaces.filter((known) => known !== path)] }));
}

export function removeBrunoWorkspace(path: string): void {
    brunoSettings.update((settings) => ({ ...settings, workspaces: settings.workspaces.filter((known) => known !== path) }));
}

/** Brings Bruno forward, loading `collectionPath` into it when given. */
export function openBrunoSession(collectionPath?: string): void {
    const settings = brunoSettings.get();
    const loaded = collectionPath ?? (settings.collectionPath || settings.workspaces[0] || "");
    if (collectionPath) registerBrunoWorkspace(collectionPath);
    const changed = loaded !== settings.collectionPath;
    if (changed) brunoSettings.update((current) => ({ ...current, collectionPath: loaded }));
    const paneId = openSurface(BRUNO_CLIENT);
    if (paneId && changed) patchView(paneId, () => DEFAULT_BRUNO_VIEW);
}

/** Asks for a collection folder, then loads it. */
export async function openBrunoFolder(): Promise<void> {
    try {
        const folder = await pickFolder("Add Bruno workspace");
        if (folder) openBrunoSession(folder);
    } catch (error) {
        reportError("add bruno workspace")(error);
    }
}

export function brunoSelectRequest(paneId: string, path: string | null): void {
    patchView(paneId, (current) => {
        if (path == null) return { ...current, activeRequestPath: null };
        const openPaths = current.openPaths.includes(path) ? current.openPaths : [...current.openPaths, path];
        return { ...current, openPaths, activeRequestPath: path };
    });
}

/** Closes a request tab; closing the active one brings a neighbour forward. Unsaved drafts are kept. */
export function brunoCloseTab(paneId: string, path: string): void {
    patchView(paneId, (current) => {
        const index = current.openPaths.indexOf(path);
        if (index === -1) return;
        const openPaths = current.openPaths.filter((open) => open !== path);
        const activeRequestPath =
            current.activeRequestPath === path ? (openPaths[Math.min(index, openPaths.length - 1)] ?? null) : current.activeRequestPath;
        return { ...current, openPaths, activeRequestPath };
    });
}

export function brunoReorderTab(paneId: string, source: string, target: string, placement: "before" | "after"): void {
    patchView(paneId, (current) => {
        const openPaths = current.openPaths.filter((open) => open !== source);
        const at = openPaths.indexOf(target);
        if (at === -1 || !current.openPaths.includes(source)) return;
        openPaths.splice(placement === "after" ? at + 1 : at, 0, source);
        return { ...current, openPaths };
    });
}

export const brunoSetReqTab = (paneId: string, reqTab: BrunoReqTab): void => patchView(paneId, (current) => ({ ...current, reqTab }));
export const brunoSetResTab = (paneId: string, resTab: BrunoResTab): void => patchView(paneId, (current) => ({ ...current, resTab }));
export const brunoSetReqPanePct = (paneId: string, reqPanePct: number): void => patchView(paneId, (current) => ({ ...current, reqPanePct }));
export const brunoToggleSecrets = (paneId: string, open?: boolean): void =>
    patchView(paneId, (current) => ({ ...current, secretsOpen: open ?? !current.secretsOpen }));

export function brunoSelectEnv(collectionPath: string, envId: string | null): void {
    brunoSettings.update((settings) => {
        const selectedEnvs = { ...settings.selectedEnvs };
        if (envId) selectedEnvs[collectionPath] = envId;
        else delete selectedEnvs[collectionPath];
        return { ...settings, selectedEnvs };
    });
}

export const brunoSetSecret = (paneId: string, name: string, value: string): void => setBrunoSecret(paneId, name, value);

/** Stash edited request text by file path; null clears the draft. */
export const brunoSetDraft = (paneId: string, path: string, text: string | null): void => setBrunoDraft(paneId, path, text);

const reloadCollection = (): void => {
    const { collectionPath } = brunoSettings.get();
    invalidate((kind, args) => kind === brunoCollectionR.kind && args[0] === collectionPath);
};

/** Writes a request's draft back to its .bru file. */
export async function brunoSaveRequest(paneId: string, path: string): Promise<void> {
    const draft = brunoDrafts(paneId)[path];
    if (draft == null) return;
    try {
        await files.writeFile(path, draft);
        brunoSetDraft(paneId, path, null);
        reloadCollection();
        notify("success", `Saved ${basename(path)}`);
    } catch (error) {
        reportError("save request")(error);
    }
}

const safeFileName = (name: string): string => name.trim().replace(/[\\/:*?"<>|]/g, "_");

export async function brunoNewRequest(paneId: string, dirPath: string, name: string): Promise<void> {
    if (!name.trim()) return;
    const file = joinPath(dirPath, `${safeFileName(name)}.bru`);
    try {
        await files.writeFileNew(file, serializeRequest(emptyRequest(name.trim())));
        reloadCollection();
        brunoSelectRequest(paneId, file);
        notify("success", `Created ${name.trim()}`);
    } catch (error) {
        reportError("create request")(error);
    }
}

export async function brunoNewFolder(parentPath: string, name: string): Promise<void> {
    if (!name.trim()) return;
    const folder = joinPath(parentPath, safeFileName(name));
    try {
        await files.createDir(folder);
        await files.writeFileNew(joinPath(folder, "folder.bru"), `meta {\n  name: ${name.trim()}\n}\n`);
        reloadCollection();
        notify("success", `Created folder ${name.trim()}`);
    } catch (error) {
        reportError("create folder")(error);
    }
}

/** Renames a request: its meta.name, and the file to match. */
export async function brunoRenameRequest(paneId: string, path: string, name: string): Promise<void> {
    if (!name.trim()) return;
    const newPath = joinPath(dirname(path), `${safeFileName(name)}.bru`);
    try {
        const text = brunoDrafts(paneId)[path] ?? (await files.readFile(path));
        const request = parseRequest(text);
        request.meta.name = name.trim();
        if (newPath === path) await files.writeFile(path, serializeRequest(request));
        else {
            await files.writeFileNew(newPath, serializeRequest(request));
            await files.deletePath(path);
        }
        brunoSetDraft(paneId, path, null);
        reloadCollection();
        patchView(paneId, (current) => ({
            ...current,
            openPaths: current.openPaths.map((open) => (open === path ? newPath : open)),
            activeRequestPath: current.activeRequestPath === path ? newPath : current.activeRequestPath,
        }));
        notify("success", `Renamed to ${name.trim()}`);
    } catch (error) {
        reportError("rename request")(error);
    }
}

export async function brunoDeleteRequest(paneId: string, path: string): Promise<void> {
    try {
        await files.deletePath(path);
        brunoSetDraft(paneId, path, null);
        reloadCollection();
        brunoCloseTab(paneId, path);
        notify("success", `Deleted ${basename(path)}`);
    } catch (error) {
        reportError("delete request")(error);
    }
}

type Palette = "requestPalette" | "environmentPalette" | "workspacePalette";

export function openPalette(palette: Palette): void {
    closeCorePalettes();
    useBruno.setState({ requestPalette: false, environmentPalette: false, workspacePalette: false, [palette]: true });
}

export function closePalettes(): void {
    useBruno.setState({ requestPalette: false, environmentPalette: false, workspacePalette: false });
}

export function togglePalette(palette: Palette): void {
    if (useBruno.getState()[palette]) closePalettes();
    else openPalette(palette);
}

const runListeners = new Set<(paneId: string) => void>();

/** Asks the pane to send its active request, as ⌘↵ does. */
export function requestRun(paneId: string): void {
    for (const listener of runListeners) listener(paneId);
}

export function onRunRequested(listener: (paneId: string) => void): () => void {
    runListeners.add(listener);
    return () => runListeners.delete(listener);
}

/** The Bruno pane in front, for a shortcut deciding whether it applies. */
export const activeBrunoPane = (): string | null => activeSurfacePane(BRUNO_CLIENT);
