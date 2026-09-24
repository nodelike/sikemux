import { fsapi } from "../api/fs";

const EXTENSIONS: Record<string, string> = {
    "image/png": "png",
    "image/jpeg": "jpg",
    "image/gif": "gif",
    "image/webp": "webp",
    "image/heic": "heic",
    "image/svg+xml": "svg",
    "image/tiff": "tiff",
    "image/bmp": "bmp",
};

// WebKit fills `files` for a drop but often leaves it empty for a paste, so `items` comes first.
export function imagesInClipboard(data: DataTransfer | null | undefined): File[] {
    if (!data) return [];
    const found: File[] = [];
    const seen = new Set<string>();
    const take = (file: File | null) => {
        if (!file || !file.type.startsWith("image/")) return;
        const key = `${file.name}:${file.size}:${file.lastModified}`;
        if (seen.has(key)) return;
        seen.add(key);
        found.push(file);
    };
    for (const item of Array.from(data.items ?? [])) {
        if (item.kind === "file") take(item.getAsFile());
    }
    for (const file of Array.from(data.files ?? [])) take(file);
    return found;
}

function extensionFor(type: string): string {
    const known = EXTENSIONS[type];
    if (known) return known;
    return type.slice(type.indexOf("/") + 1).replace(/[^a-z0-9]/gi, "") || "png";
}

// A pasted screenshot is always called `image.png`, so it is named after the moment instead.
export function attachmentName(file: File, now = new Date()): string {
    const placeholder = !file.name || /^(image|screenshot)\.\w+$/i.test(file.name);
    if (!placeholder) return file.name;
    const pad = (value: number, width = 2) => String(value).padStart(width, "0");
    const stamp = `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}-${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}${pad(now.getMilliseconds(), 3)}`;
    return `pasted-${stamp}.${extensionFor(file.type)}`;
}

// Chunked: one `String.fromCharCode` over a whole screenshot overflows the argument list.
export function base64Of(bytes: Uint8Array): string {
    const CHUNK = 0x8000;
    let binary = "";
    for (let at = 0; at < bytes.length; at += CHUNK) {
        binary += String.fromCharCode(...bytes.subarray(at, at + CHUNK));
    }
    return btoa(binary);
}

/* WKWebView often hands over an empty event for a pasted screenshot, so the
   system clipboard is asked then — but never for a text paste, whose clipboard
   can carry a picture too, such as a Finder file's icon. */
export async function savePastedClipboard(data: DataTransfer | null | undefined): Promise<string[]> {
    const fromEvent = imagesInClipboard(data);
    if (fromEvent.length > 0) return savePastedImages(fromEvent);
    if (Array.from(data?.types ?? []).some((type) => type.startsWith("text/"))) return [];

    const saved = await fsapi.saveClipboardImage(attachmentName(new File([], "image.png", { type: "image/png" })));
    return saved ? [saved] : [];
}

export async function savePastedImages(files: readonly File[]): Promise<string[]> {
    if (files.length === 0) return [];
    const dir = await fsapi.chatAttachmentDir();
    const saved: string[] = [];
    for (const file of files) {
        const bytes = new Uint8Array(await file.arrayBuffer());
        saved.push(await fsapi.saveBase64IntoDir(dir, attachmentName(file), base64Of(bytes)));
    }
    return saved;
}
