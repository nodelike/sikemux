import { extensionOf, fileUriToPath } from "./paths";

const IMAGE_TYPES: Record<string, string> = {
    png: "image/png",
    jpg: "image/jpeg",
    jpeg: "image/jpeg",
    gif: "image/gif",
    webp: "image/webp",
    svg: "image/svg+xml",
    bmp: "image/bmp",
    ico: "image/x-icon",
    avif: "image/avif",
    tif: "image/tiff",
    tiff: "image/tiff",
    heic: "image/heic",
    heif: "image/heif",
};

/** Pictures a transcript can draw on any client; others go as files. */
const DRAWN_TYPES = new Set(["image/jpeg", "image/png", "image/gif", "image/webp"]);

/** The picture type a file's name says it holds, if it says one. */
export function imageType(path: string): string | undefined {
    return IMAGE_TYPES[extensionOf(path)];
}

export function isDrawnType(mime: string): boolean {
    return DRAWN_TYPES.has(mime);
}

export function isImagePath(path: string | null | undefined): boolean {
    return !!path && imageType(path) !== undefined;
}

export function localPath(uri: string | null | undefined): string | null {
    if (!uri) return null;
    if (!uri.startsWith("file://")) return uri.startsWith("/") ? uri : null;
    return fileUriToPath(uri);
}

export function localImagePath(uri: string | null | undefined): string | null {
    const path = localPath(uri);
    return path && isImagePath(path) ? path : null;
}
