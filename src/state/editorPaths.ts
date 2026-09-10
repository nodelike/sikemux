import { fsapi } from "../api/fs";
import { isPathWithin, joinPath, relativePath } from "../lib/paths";
import { emit } from "./bus";
import { mutate } from "./store";

export function relocatedPath(path: string, src: string, dest: string): string {
    if (path === src) return dest;
    return isPathWithin(path, src) ? joinPath(dest, relativePath(path, src) ?? "") : path;
}

export async function renameEditorPath(src: string, dest: string): Promise<void> {
    await fsapi.rename(src, dest);
    emit({ type: "path-renamed", src, dest });
    mutate((state) => {
        for (const view of Object.values(state.editorViews)) {
            view.openTabs = view.openTabs.map((path) => relocatedPath(path, src, dest));
            if (view.activePath) view.activePath = relocatedPath(view.activePath, src, dest);
        }
        for (const [paneId, paths] of Object.entries(state.dirtyEditorPaths))
            state.dirtyEditorPaths[paneId] = paths.map((path) => relocatedPath(path, src, dest));
    });
}
