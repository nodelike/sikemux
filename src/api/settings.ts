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
    scanProjectRoots: (roots: ProjectRoot[]) => invoke<ProjectEntry[]>("scan_project_roots", { roots }),
    expandPath: (path: string) => invoke<string>("expand_path", { path }),
    isDirectory: (path: string) => invoke<boolean>("is_directory", { path }),
    pickFolder: (defaultPath?: string) => dialogsApi.pickFolder({ defaultPath }),
    pickImage: () =>
        dialogsApi.pickFile({
            filters: [{ name: "Images", extensions: ["png", "jpg", "jpeg", "webp", "gif", "avif", "heic", "bmp", "tiff"] }],
        }),
};
