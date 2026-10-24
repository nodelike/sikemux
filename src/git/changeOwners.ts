import { DIFF_ROW, type ChangeAuthor, type ChangeOwners, type DiffRow, type FileOwners, type LineOwners } from "../api/git";
import { useCachedResourceEnabled } from "../state/resources";
import { gitChangeOwnersR } from "../state/resources.defs";

/** A file's owned lines with the names of the agents they point at. */
export interface OwnedFile {
    file: FileOwners;
    names: readonly string[];
}

/** Who wrote a checkout's uncommitted lines, or its staged ones, read again whenever git reports a change. */
export function useChangeOwners(repo: string, staged: boolean, enabled = true): ChangeOwners | undefined {
    return useCachedResourceEnabled(enabled && !!repo, gitChangeOwnersR, repo, staged).data;
}

const ownedFiles = new WeakMap<LineOwners, Map<string, OwnedFile>>();

/** A file's owned lines, the same object for as long as `owners` is. */
export function ownedFile(owners: LineOwners | undefined, path: string): OwnedFile | undefined {
    if (!owners) return undefined;
    let files = ownedFiles.get(owners);
    if (!files) {
        const names = owners.authors.map((author) => author.name);
        files = new Map(owners.files.map((file) => [file.path, { file, names }]));
        ownedFiles.set(owners, files);
    }
    return files.get(path);
}

/** The agents that wrote some of a file's uncommitted changes, in the order they first appear. */
export function fileAuthors(owners: ChangeOwners | undefined, path: string): ChangeAuthor[] {
    const owned = ownedFile(owners, path);
    if (!owners || !owned) return [];
    const authors = new Set([...owned.file.lines.map((run) => run.author), ...owned.file.removed.map((gap) => gap.author)]);
    return [...authors].map((author) => owners.authors[author]);
}

/**
 * The notes the core says to show beside a file's changed lines, in this app's words, by line; `gaps` holds the
 * writer of lines taken out just before a line.
 */
export function fileNotes(owned: OwnedFile | undefined): { lines: Map<number, string>; gaps: Map<number, string> } {
    const lines = new Map<number, string>();
    const gaps = new Map<number, string>();
    if (!owned) return { lines, gaps };
    const { file, names } = owned;
    for (const note of file.notes ?? []) {
        const parts: string[] = [];
        if (note.named) parts.push(note.author === undefined ? "unattributed" : names[note.author]);
        if (note.over !== undefined) parts.push(`changed ${names[note.over]}'s edit`);
        if (parts.length) lines.set(note.line, parts.join(" · "));
    }
    for (const removal of file.removed) gaps.set(removal.at, names[removal.author]);
    return { lines, gaps };
}

/** The notes for a unified diff's rows: an added row's by its line, and lines taken out with nothing in their place on the first of them. */
export function diffRowNotes(rows: readonly DiffRow[], owned: OwnedFile | undefined): (string | undefined)[] | null {
    const { lines, gaps } = fileNotes(owned);
    if (!lines.size && !gaps.size) return null;
    const notes: (string | undefined)[] = rows.map((row) => (row[0] === DIFF_ROW.added ? lines.get(row[1]) : undefined));
    let lastLine = 0;
    let open: { row: number; start: number; added: boolean } | null = null;
    const close = () => {
        if (open && !open.added) notes[open.row] = gaps.get(open.start);
        open = null;
    };
    rows.forEach((row, index) => {
        const [kind] = row;
        if (kind === DIFF_ROW.added || kind === DIFF_ROW.deleted) {
            open ??= { row: index, start: lastLine + 1, added: false };
            if (kind === DIFF_ROW.added) {
                open.added = true;
                lastLine = row[1];
            }
            return;
        }
        close();
        if (kind === DIFF_ROW.context) lastLine = row[1];
    });
    close();
    return notes;
}
