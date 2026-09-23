import { invokeCommand as invoke } from "./invoke";

export interface DirEntry {
    name: string;
    path: string;
    is_dir: boolean;
}

export interface FileBlob {
    mime: string;
    data: string;
    size: number;
}

export interface FileSnapshot {
    content: string;
    version: string;
}

export interface DirListing {
    path: string;
    entries: DirEntry[];
    error: string | null;
}

export interface FileWriteResult {
    version: string;
}

export type PathKind = "file" | "dir";

export const fsapi = {
    readDir: (path: string) => invoke<DirEntry[]>("read_dir", { path }),
    readDirs: (paths: string[]) => invoke<DirListing[]>("read_dirs", { paths }),
    pathKinds: (paths: string[]) => invoke<(PathKind | null)[]>("path_kinds", { paths }),
    readFile: (path: string) => invoke<string>("read_file", { path }),
    readFileVersioned: (path: string) => invoke<FileSnapshot>("read_file_versioned", { path }),
    readTextFileLimited: (path: string) => invoke<string>("read_text_file_limited", { path }),
    readFileBase64: (path: string) => invoke<FileBlob>("read_file_base64", { path }),
    writeFile: (path: string, content: string) => invoke<void>("write_file", { path, content }),
    writeFileVersioned: (path: string, content: string, expectedVersion: string) =>
        invoke<FileWriteResult>("write_file_versioned", { path, content, expectedVersion }),
    writeFileNew: (path: string, content: string) => invoke<void>("write_file_new", { path, content }),
    createFile: (path: string) => invoke<void>("create_file", { path }),
    createDir: (path: string) => invoke<void>("create_dir", { path }),
    copyIntoDir: (src: string, dir: string) => invoke<string>("copy_into_dir", { src, dir }),
    downloadsDir: () => invoke<string>("downloads_dir"),
    chatAttachmentDir: () => invoke<string>("chat_attachment_dir"),
    clipboardPng: () => invoke<string | null>("clipboard_png"),
    saveBase64IntoDir: (dir: string, name: string, data: string) => invoke<string>("save_base64_into_dir", { dir, name, data }),
    rename: (src: string, dest: string) => invoke<void>("rename_path", { src, dest }),
    revealInFinder: (path: string) => invoke<void>("reveal_in_finder", { path }),
    deletePath: (path: string) => invoke<void>("delete_path", { path }),
};
