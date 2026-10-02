import { rankBy } from "../lib/fuzzy";
import { basename } from "../lib/paths";

export const MAX_ATTACHMENTS = 32;

export function mergePaths(current: string[], incoming: readonly string[]): string[] {
    const merged = [...current];
    for (const path of incoming) {
        if (!path || path.includes("\0") || merged.includes(path)) continue;
        if (merged.length === MAX_ATTACHMENTS) break;
        merged.push(path);
    }
    return merged;
}

/** `/` runs a command, `@` names something in the project, `#` names an issue or pull request. */
export type ComposerTrigger = "/" | "@" | "#";

export interface ComposerToken {
    trigger: ComposerTrigger;
    start: number;
    needle: string;
}

/* The token a draft is naming is the one the caret sits in, so a trigger works
   part-way through a sentence and not only as the first thing typed. It has to
   start a word, which keeps `a/b`, `me@x.com` and `C#` plain text. */
export function tokenAt(text: string, caret: number): ComposerToken | null {
    for (let start = caret - 1; start >= 0; start -= 1) {
        const character = text[start];
        if (/\s/.test(character)) return null;
        if (character !== "/" && character !== "@" && character !== "#") continue;
        if (start > 0 && !/\s/.test(text[start - 1])) continue;
        return { trigger: character, start, needle: text.slice(start + 1, caret) };
    }
    return null;
}

/** The draft once a chosen token is taken out of it, and where the caret lands. */
export function removeToken(text: string, token: Pick<ComposerToken, "start">, caret: number): { text: string; caret: number } {
    const before = text.slice(0, token.start);
    let after = text.slice(caret);
    if ((before === "" || /\s$/.test(before)) && /^ /.test(after)) after = after.slice(1);
    return { text: `${before}${after}`, caret: before.length };
}

export interface ProjectEntry {
    /** Relative to the project, with a trailing slash on a folder. */
    path: string;
    folder: boolean;
}

/** Every file the project lists, then every folder that holds one, so a file wins when names tie. */
export function projectEntries(files: readonly string[]): ProjectEntry[] {
    const folders = new Set<string>();
    for (const file of files) {
        for (let slash = file.indexOf("/"); slash > 0; slash = file.indexOf("/", slash + 1)) folders.add(file.slice(0, slash + 1));
    }
    return [...files.map((path) => ({ path, folder: false })), ...[...folders].sort().map((path) => ({ path, folder: true }))];
}

export function entryName(entry: ProjectEntry): string {
    return basename(entry.path);
}

export function rankEntries(needle: string, entries: readonly ProjectEntry[], limit: number): ProjectEntry[] {
    return rankBy(needle, entries, (entry) => [entryName(entry), entry.path], limit);
}
