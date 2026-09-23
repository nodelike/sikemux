import { fsapi } from "../api/fs";

/* Clipboard types the systems people paste from actually produce. Anything else
   that calls itself an image still lands, under the subtype it declared. */
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

/* `items` is read first and `files` only as a fallback: WebKit fills `files`
   for a drop but frequently leaves it empty for a paste, handing the picture
   over as an item instead. Reading only `files` is why pasting looked like it
   did nothing. */
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

/**
 * What the picture is called once it is a file. A screenshot pasted from the
 * clipboard arrives as the placeholder `image.png` every time, so it is given
 * the moment instead — two pastes in one conversation should not read as the
 * same attachment.
 */
export function attachmentName(file: File, now = new Date()): string {
    const placeholder = !file.name || /^(image|screenshot)\.\w+$/i.test(file.name);
    if (!placeholder) return file.name;
    const pad = (value: number, width = 2) => String(value).padStart(width, "0");
    const stamp = `${now.getFullYear()}${pad(now.getMonth() + 1)}${pad(now.getDate())}-${pad(now.getHours())}${pad(now.getMinutes())}${pad(now.getSeconds())}${pad(now.getMilliseconds(), 3)}`;
    return `pasted-${stamp}.${extensionFor(file.type)}`;
}

/* Chunked because a screenshot is megabytes, and one `String.fromCharCode`
   over the whole buffer overflows the argument list. */
export function base64Of(bytes: Uint8Array): string {
    const CHUNK = 0x8000;
    let binary = "";
    for (let at = 0; at < bytes.length; at += CHUNK) {
        binary += String.fromCharCode(...bytes.subarray(at, at + CHUNK));
    }
    return btoa(binary);
}

/**
 * What the paste actually attaches. The DOM event is tried first, and AppKit
 * asked only when it turned up nothing: WKWebView often hands a paste handler
 * an event with neither files nor items on it, which is why pasting a
 * screenshot appeared to do nothing at all.
 */
export async function savePastedClipboard(data: DataTransfer | null | undefined): Promise<string[]> {
    const fromEvent = imagesInClipboard(data);
    if (fromEvent.length > 0) return savePastedImages(fromEvent);

    const png = await fsapi.clipboardPng();
    if (!png) return [];
    const dir = await fsapi.chatAttachmentDir();
    return [await fsapi.saveBase64IntoDir(dir, attachmentName(new File([], "image.png", { type: "image/png" })), png)];
}

/** Writes pasted pictures into the app's scratch directory, newest name first. */
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
