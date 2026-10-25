import { MAX_ATTACHMENTS } from "../wire.generated";
import { rankBy } from "../lib/fuzzy";
import { basename } from "../lib/paths";
import type { ChatCommandInfo } from "./types";

export function mergePaths(current: string[], incoming: readonly string[]): string[] {
    const merged = [...current];
    for (const path of incoming) {
        if (!path || path.includes("\0") || merged.includes(path)) continue;
        if (merged.length === MAX_ATTACHMENTS) break;
        merged.push(path);
    }
    return merged;
}

const MAX_COMMANDS = 8;

/** The agent's slash commands with `needle` anywhere in their name. */
export function commandMatches(commands: readonly ChatCommandInfo[], needle: string): ChatCommandInfo[] {
    const lower = needle.toLowerCase();
    return commands.filter((command) => command.name.toLowerCase().includes(lower)).slice(0, MAX_COMMANDS);
}

/** The command a message opens with that the session does not offer. A session that lists none is taken at its word. */
export function unknownCommand(text: string, commands: readonly ChatCommandInfo[]): string | null {
    const name = /^\/(\S+)/.exec(text)?.[1];
    return name && commands.length > 0 && !commands.some((command) => command.name === name) ? name : null;
}

/** `/` runs a command, `@` names an agent or earlier chat, `#` an issue, `!` a pull request and `+` a file or folder. */
export type ComposerTrigger = "/" | "@" | "#" | "!" | "+";

const TRIGGERS: readonly string[] = ["/", "@", "#", "!", "+"];

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
        if (!TRIGGERS.includes(character)) continue;
        if (start > 0 && !/\s/.test(text[start - 1])) continue;
        return { trigger: character as ComposerTrigger, start, needle: text.slice(start + 1, caret) };
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

/** Where a tag written into the draft starts, when the caret sits right after it. */
export function tagBefore(text: string, caret: number, tags: readonly string[]): number | null {
    for (const tag of tags) {
        const start = caret - tag.length;
        if (start >= 0 && text.slice(start, caret) === tag && (start === 0 || /\s/.test(text[start - 1]))) return start;
    }
    return null;
}

/** Where a tag is written in the draft as a word of its own. */
export function tagStarts(text: string, tag: string): number[] {
    const starts: number[] = [];
    for (let at = text.indexOf(tag); at !== -1; at = text.indexOf(tag, at + 1)) {
        const end = at + tag.length;
        if ((at === 0 || /\s/.test(text[at - 1])) && (end === text.length || /[\s.,;:!?)]/.test(text[end]))) starts.push(at);
    }
    return starts;
}

export function hasTag(text: string, tag: string): boolean {
    return tagStarts(text, tag).length > 0;
}

/** The most rows the composer's menu lists for files, agents or issues. */
export const MAX_MENU_ENTRIES = 8;

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

/** How the draft names attachment `number`, counting from 1, at the place its picture belongs. */
export const imageTag = (number: number): string => `[Image #${number}]`;

/** The draft once attachment `index` is gone: its tags go, and later ones count down to keep naming theirs. */
export function withoutAttachment(text: string, index: number): string {
    const gone = index + 1;
    return text
        .replace(new RegExp(` ?\\[Image #${gone}\\]`, "g"), "")
        .replace(/\[Image #(\d+)\]/g, (tag, number: string) => (Number(number) > gone ? imageTag(Number(number) - 1) : tag));
}
