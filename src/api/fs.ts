import { coreCall } from "./core";
import { invokeCommand as invoke } from "./invoke";
import { joinPath } from "../lib/paths";

export interface DirEntry {
    name: string;
    path: string;
    is_dir: boolean;
    ignored?: boolean;
}

export interface FilePreview {
    mime: string;
    size: number;
    modified: number;
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

/** A host on the account by its key, or undefined for this one. */
export type FilesHost = string | undefined;

interface WireListing {
    readonly dir: string;
    readonly entries: { readonly name: string; readonly dir?: boolean; readonly ignored?: boolean }[];
    readonly total: number;
    readonly error?: string;
}

interface Piece {
    readonly text: string;
    readonly next: number;
    readonly stamp: string;
    readonly version?: string;
}

/** Characters per piece of a save to another host: at most 1.5 MB however they encode. */
const SAVE_PIECE = 512 * 1024;
const READ_TRIES = 3;

function call<T>(host: FilesHost, method: string, params: unknown): Promise<T> {
    return coreCall<T>(method, params, host);
}

async function readDirs(host: FilesHost, dirs: readonly string[]): Promise<DirListing[]> {
    const listings = await call<WireListing[]>(host, "files.list", { dirs });
    return Promise.all(
        listings.map(async (listing) => {
            const entries: DirEntry[] = [];
            const add = (page: WireListing) => {
                for (const entry of page.entries) {
                    entries.push({
                        name: entry.name,
                        path: joinPath(page.dir, entry.name),
                        is_dir: !!entry.dir,
                        ...(entry.ignored ? { ignored: true } : {}),
                    });
                }
            };
            add(listing);
            while (!listing.error && entries.length < listing.total) {
                const [page] = await call<WireListing[]>(host, "files.list", { dirs: [listing.dir], from: entries.length });
                if (!page || page.error || page.entries.length === 0) break;
                add(page);
            }
            return { path: listing.dir, entries, error: listing.error ?? null };
        }),
    );
}

/** The file as text with its version, read in pieces, and again from the start if it moves between them. */
async function readFileVersioned(host: FilesHost, path: string): Promise<FileSnapshot> {
    for (let attempt = 1; ; attempt++) {
        let content = "";
        let at = 0;
        let stamp: string | undefined;
        for (;;) {
            const piece = await call<Piece>(host, "files.read", { path, at });
            if (stamp !== undefined && piece.stamp !== stamp) break;
            stamp = piece.stamp;
            content += piece.text;
            at = piece.next;
            if (piece.version !== undefined) return { content, version: piece.version };
        }
        if (attempt === READ_TRIES) throw new Error(`${path} kept changing while it was read`);
    }
}

function pieces(content: string): string[] {
    const out: string[] = [];
    let at = 0;
    do {
        let end = Math.min(content.length, at + SAVE_PIECE);
        const last = content.charCodeAt(end - 1);
        if (end < content.length && last >= 0xd800 && last <= 0xdbff) end -= 1;
        out.push(content.slice(at, end));
        at = end;
    } while (at < content.length);
    return out;
}

/** Saves over the file at `version`, as a new file with `new`, or over whatever is there with neither. */
async function write(host: FilesHost, path: string, content: string, options: { version?: string; new?: boolean }): Promise<FileWriteResult> {
    const parts = pieces(content);
    let upload: string | null = null;
    for (let index = 0; ; index++) {
        const more = index < parts.length - 1;
        const written: { version?: string; upload?: string } = await call(host, "files.write", {
            path,
            text: parts[index],
            version: options.version ?? null,
            new: !!options.new,
            upload,
            more,
        });
        if (!more) return { version: written.version! };
        upload = written.upload ?? null;
    }
}

/** What the file tree and the editor do on the disk of the host that holds a project's copy. */
export function filesOn(host: FilesHost) {
    return {
        readDir: (path: string) =>
            readDirs(host, [path]).then(([listing]) => {
                if (listing.error) throw new Error(listing.error);
                return listing.entries;
            }),
        readDirs: (paths: string[]) => readDirs(host, paths),
        pathKinds: (paths: string[]) => call<(PathKind | null)[]>(host, "files.kinds", { paths }),
        readFile: (path: string) => readFileVersioned(host, path).then((snapshot) => snapshot.content),
        readFileVersioned: (path: string) => readFileVersioned(host, path),
        writeFile: (path: string, content: string) => write(host, path, content, {}).then(() => undefined),
        writeFileVersioned: (path: string, content: string, expectedVersion: string) => write(host, path, content, { version: expectedVersion }),
        writeFileNew: (path: string, content: string) => write(host, path, content, { new: true }).then(() => undefined),
        createFile: (path: string) => call<void>(host, "files.create", { path }),
        createDir: (path: string) => call<void>(host, "files.create", { path, dir: true }),
        rename: (src: string, dest: string) => call<void>(host, "files.rename", { from: src, to: dest }),
        deletePath: (path: string) => call<void>(host, "files.delete", { path }),
    };
}

export type HostFiles = ReturnType<typeof filesOn>;

/** The largest file an inline diff reads whole. */
const INLINE_TEXT_LIMIT = 1024 * 1024;

export const fsapi = {
    ...filesOn(undefined),
    keepDroppedPaths: (paths: string[]) => invoke<string[]>("keep_dropped_paths", { paths }),
    readTextFileLimited: (path: string) => coreCall<string>("files.readText", { path, limit: INLINE_TEXT_LIMIT }),
    previewFile: (path: string) => invoke<FilePreview>("preview_file", { path }),
    copyIntoDir: (src: string, dir: string) => coreCall<string>("files.copyInto", { from: src, dir }),
    downloadsDir: () => invoke<string>("downloads_dir"),
    chatAttachmentDir: () => invoke<string>("chat_attachment_dir"),
    saveClipboardImage: (name: string) => invoke<string | null>("save_clipboard_image", { name }),
    saveBase64IntoDir: (dir: string, name: string, data: string) => coreCall<string>("files.writeBytes", { dir, name, base64: data }),
    openInDefaultApp: (path: string) => invoke<void>("open_in_default_app", { path }),
    revealInFinder: (path: string) => invoke<void>("reveal_in_finder", { path }),
};
