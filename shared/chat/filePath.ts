import { EXTENSIONLESS } from "../wire.generated";
import { expandHome, fileUriToPath, joinPath, normalizePath } from "../lib/paths";

/** A reference to a file, resolved far enough to open it. */
export interface PathRef {
    /** Absolute, with `/` separators. */
    path: string;
    /** 1-based, as a reader would count them, when the reference named one. */
    line?: number;
    column?: number;
}

export interface PathRoots {
    cwd: string;
    home?: string;
}

/* A reference no longer than this is worth checking; anything longer is prose
   that happens to contain slashes. */
const MAX_REF_LENGTH = 512;

/* http:, mailto:, data: and friends. A Windows drive letter is one character,
   which is why a scheme here needs at least two. */
const URL_SCHEME = /^[a-z][a-z0-9+.-]+:/i;
const WINDOWS_DRIVE = /^[A-Za-z]:[\\/]/;

/* `path:12`, `path:12:5`, `path#L12` — how a tool, a compiler and a code host
   each write the same thing. */
const LINE_SUFFIX = /(?::(\d+))(?::(\d+))?$/;
const ANCHOR_SUFFIX = /#L(\d+)(?:C(\d+))?$/i;

function splitLocation(raw: string): { body: string; line?: number; column?: number } {
    const anchor = ANCHOR_SUFFIX.exec(raw);
    if (anchor) return { body: raw.slice(0, anchor.index), line: Number(anchor[1]), column: anchor[2] ? Number(anchor[2]) : undefined };
    const suffix = LINE_SUFFIX.exec(raw);
    if (suffix) return { body: raw.slice(0, suffix.index), line: Number(suffix[1]), column: suffix[2] ? Number(suffix[2]) : undefined };
    return { body: raw };
}

/* A bare name is only a file reference when it carries an extension or is one
   of the few names a project spells out in full. Everything else needs a
   separator to tell it apart from an ordinary word. */
const EXTENSIONLESS_NAMES = new Set(EXTENSIONLESS);
export const HAS_EXTENSION = /\.[A-Za-z0-9]{1,8}$/;

function looksRelative(body: string): boolean {
    if (body.includes("/") || body.includes("\\")) return true;
    if (body.startsWith(".")) return true;
    return HAS_EXTENSION.test(body) || EXTENSIONLESS_NAMES.has(body.toLowerCase());
}

/**
 * Reads a string an agent wrote as a file reference, or null when it is not
 * one. Existence is a separate question — this only decides shape.
 */
export function parsePathRef(raw: string, roots: PathRoots): PathRef | null {
    const trimmed = raw.trim();
    if (!trimmed || trimmed.length > MAX_REF_LENGTH) return null;

    if (/^file:\/\//i.test(trimmed)) {
        const { body, line, column } = splitLocation(trimmed);
        const decoded = fileUriToPath(body);
        return decoded ? { path: normalizePath(decoded), line, column } : null;
    }

    const { body, line, column } = splitLocation(trimmed);
    if (!body) return null;
    if (URL_SCHEME.test(body) && !WINDOWS_DRIVE.test(body)) return null;

    const expanded = expandHome(body, roots.home);
    const absolute = expanded.startsWith("/") || WINDOWS_DRIVE.test(expanded);
    if (!absolute && /\s/.test(body)) return null;
    if (!absolute && !looksRelative(expanded)) return null;
    if (!absolute && !roots.cwd) return null;

    const path = absolute ? normalizePath(expanded) : normalizePath(joinPath(roots.cwd, expanded.replace(/^\.\//, "")));
    return path ? { path, line, column } : null;
}
