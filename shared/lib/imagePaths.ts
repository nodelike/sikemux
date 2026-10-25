import { extensionOf, fileUriToPath } from "./paths";

export const IMAGE_EXTS = ["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "ico", "avif", "tif", "tiff", "heic", "heif"];

export function isImagePath(path: string | null | undefined): boolean {
    return !!path && IMAGE_EXTS.includes(extensionOf(path));
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
