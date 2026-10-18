import { invokeCommand as invoke } from "./invoke";

export interface FileFilter {
    name: string;
    extensions: string[];
}

interface PickOptions {
    directory?: boolean;
    multiple?: boolean;
    title?: string;
    defaultPath?: string;
    filters?: FileFilter[];
}

const pickPaths = (options: PickOptions) => invoke<string[] | null>("pick_paths", { options });
const first = (paths: string[] | null) => paths?.[0] ?? null;

export const dialogsApi = {
    pickFolder: async (options: { title?: string; defaultPath?: string } = {}) => first(await pickPaths({ ...options, directory: true })),
    pickFile: async (options: { title?: string; filters?: FileFilter[] } = {}) => first(await pickPaths(options)),
    pickFiles: async () => (await pickPaths({ multiple: true })) ?? [],
    pickSavePath: (fileName: string, options: { title?: string; filters?: FileFilter[] } = {}) =>
        invoke<string | null>("pick_save_path", { fileName, title: options.title ?? null, filters: options.filters ?? [] }),
};
