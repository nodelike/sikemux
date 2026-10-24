import { coreCall } from "./core";
import { invokeCommand as invoke } from "./invoke";
import { dialogsApi } from "./dialogs";
import type { ProjectRoot } from "../state/types";

export interface ProjectEntry {
    name: string;
    path: string;
}

export interface Wallpaper {
    name: string;
    dataUrl: string;
}

export const settingsApi = {
    wallpaperImage: () => invoke<Wallpaper>("wallpaper_image"),
    scanProjectRoots: (roots: ProjectRoot[]) => coreCall<ProjectEntry[]>("projectRoots", { roots }),
    /** The path with `~` expanded, and what is there. */
    resolvePath: (path: string) => coreCall<{ path: string; kind: "file" | "dir" | null }>("resolvePath", { path }),
    pickFolder: (defaultPath?: string) => dialogsApi.pickFolder({ defaultPath }),
    pickImage: () =>
        dialogsApi.pickFile({
            filters: [{ name: "Images", extensions: ["png", "jpg", "jpeg", "webp", "gif", "avif", "heic", "bmp", "tiff"] }],
        }),
};
