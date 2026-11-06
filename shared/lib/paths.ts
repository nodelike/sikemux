export function basename(path: string): string {
    return (
        path
            .replace(/[\\/]+$/, "")
            .split(/[\\/]/)
            .pop() || path
    );
}

export function dirname(path: string): string {
    const trimmed = path.replace(/[\\/]+$/, "");
    const i = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
    return i < 0 ? "" : trimmed.slice(0, i);
}

export function normalizePath(path: string): string {
    return path.replaceAll("\\", "/").replace(/\/+$/, "");
}

export function joinPath(base: string, ...parts: string[]): string {
    return [normalizePath(base), ...parts.map((part) => part.replaceAll("\\", "/").replace(/^\/+|\/+$/g, ""))].filter(Boolean).join("/");
}

export function expandHome(path: string, home: string | null | undefined): string {
    if (!home) return path;
    if (path === "~") return home;
    if (path.startsWith("~/") || path.startsWith("~\\")) return `${home}${path.slice(1)}`;
    return path;
}

/** The path a `file:` URI names, or null when it is not one. A Windows drive loses the slash in front of it. */
export function fileUriToPath(uri: string): string | null {
    if (!/^file:/i.test(uri)) return null;
    let url: URL;
    let pathname: string;
    try {
        url = new URL(uri);
        pathname = decodeURIComponent(url.pathname);
    } catch {
        return null;
    }
    if (url.hostname && url.hostname.toLowerCase() !== "localhost") return `//${url.hostname}${pathname}`;
    return /^\/[A-Za-z]:/.test(pathname) ? pathname.slice(1) : pathname;
}

/** A file's extension in lower case, without its dot. A hidden file's leading dot starts its name, not an extension. */
export function extensionOf(path: string): string {
    const name = path.toLowerCase().split(/[\\/]/).pop() ?? "";
    const dot = name.lastIndexOf(".");
    return dot > 0 ? name.slice(dot + 1) : "";
}
