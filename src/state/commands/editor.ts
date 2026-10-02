import { emit } from "../bus";
import { mutate, type StoreState } from "../store";
import { ensureRoleWindow } from "./shared";

export function requestOpenFile(path: string, line?: number, character?: number): void {
    ensureRoleWindow("files", "editor", "editor", path);
    emit({ type: "open-file", path, line, character });
}

export const openEditorPane = (): void => ensureRoleWindow("files", "editor", "editor");

/** Opens `path` as a tab, and returns the preview tab it took the place of, if any. */
export function openEditorTab(paneId: string, path: string, activate = true, preview = false): string | null {
    let replaced: string | null = null;
    mutate((d) => {
        const cur = d.editorViews[paneId] ?? { openTabs: [], activePath: null };
        const previewIndex = cur.preview ? cur.openTabs.indexOf(cur.preview) : -1;
        if (cur.openTabs.includes(path)) {
            if (!preview && cur.preview === path) delete cur.preview;
        } else if (preview && previewIndex >= 0) {
            replaced = cur.openTabs[previewIndex];
            cur.openTabs[previewIndex] = path;
            if (cur.activePath === replaced) cur.activePath = path;
            cur.preview = path;
        } else {
            cur.openTabs.push(path);
            if (preview) cur.preview = path;
        }
        if (activate) cur.activePath = path;
        d.editorViews[paneId] = cur;
    });
    return replaced;
}

export function keepEditorTab(paneId: string, path: string): void {
    mutate((d) => {
        const cur = d.editorViews[paneId];
        if (cur?.preview === path) delete cur.preview;
    });
}

export function setEditorView(paneId: string, patch: Partial<StoreState["editorViews"][string]>): void {
    mutate((d) => {
        const cur = d.editorViews[paneId] ?? {
            openTabs: [],
            activePath: null,
        };
        d.editorViews[paneId] = { ...cur, ...patch };
    });
}

export function setEditorDirtyPaths(paneId: string, paths: string[]): void {
    mutate((d) => {
        if (paths.length === 0) delete d.dirtyEditorPaths[paneId];
        else d.dirtyEditorPaths[paneId] = paths;
    });
}
