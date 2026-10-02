import { convertFileSrc } from "@tauri-apps/api/core";

export type ViewerKind = "image" | "video" | "audio" | "pdf" | "font" | "document" | "binary";

const IMAGE_EXTS = ["png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "ico", "avif", "tif", "tiff", "heic", "heif"];

/* Extensions that are never text, so the editor goes straight to a viewer
   instead of reading them first. Anything not named here that turns out not
   to be text is still previewed; this only saves the failed read. */
const PREVIEW_EXTS = new Set([
    ...IMAGE_EXTS,
    ...["psd", "pdf", "ttf", "otf", "woff", "woff2"],
    ...["mp4", "m4v", "mov", "webm", "mkv", "avi"],
    ...["mp3", "m4a", "aac", "wav", "flac", "aif", "aiff", "ogg", "oga", "opus"],
    ...["doc", "docx", "xls", "xlsx", "ppt", "pptx", "rtf", "odt", "ods", "odp", "pages", "numbers", "epub"],
    ...["zip", "gz", "tgz", "bz2", "xz", "7z", "rar", "tar", "jar", "dmg", "iso"],
    ...["exe", "dll", "so", "dylib", "o", "a", "class", "pyc", "wasm", "sqlite", "sqlite3"],
]);

const DOCUMENT_MIMES = new Set([
    "application/rtf",
    "application/msword",
    "application/vnd.ms-excel",
    "application/vnd.ms-powerpoint",
    "application/vnd.apple.pages",
    "application/vnd.apple.numbers",
    "application/vnd.apple.keynote",
    "application/epub+zip",
    "image/vnd.adobe.photoshop",
]);

/* WebKit has no decoder for these containers. */
const UNPLAYABLE_VIDEO = new Set(["video/x-matroska", "video/x-msvideo"]);

export function extname(path: string): string {
    const file = path.split("/").pop()?.toLowerCase() ?? "";
    const i = file.lastIndexOf(".");
    return i > 0 ? file.slice(i + 1) : "";
}

export function isImagePath(path: string | null | undefined): boolean {
    return !!path && IMAGE_EXTS.includes(extname(path));
}

export function isPreviewPath(path: string | null | undefined): boolean {
    return !!path && PREVIEW_EXTS.has(extname(path));
}

export function viewerKind(mime: string): ViewerKind {
    if (DOCUMENT_MIMES.has(mime) || mime.startsWith("application/vnd.openxmlformats-") || mime.startsWith("application/vnd.oasis.opendocument."))
        return "document";
    if (mime === "application/pdf") return "pdf";
    if (mime.startsWith("image/")) return "image";
    if (mime.startsWith("audio/")) return "audio";
    if (mime.startsWith("video/")) return UNPLAYABLE_VIDEO.has(mime) ? "binary" : "video";
    if (mime.startsWith("font/")) return "font";
    return "binary";
}

/** Where the window loads a file `preview_file` has granted. */
export function previewUrl(path: string, revision = 0): string {
    const url = convertFileSrc(path, "preview");
    return revision ? `${url}?v=${revision}` : url;
}
