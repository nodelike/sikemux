import { DIFF_ROW, type ChangeAuthor, type ChangeOwners, type CommitOwner, type DiffRow, type FileOwners, type OwnedLines } from "../api/git";
import { useCachedResourceEnabled } from "../state/resources";
import { gitChangeOwnersR } from "../state/resources.defs";

/** A file's owned lines with the names of the agents they point at. */
export interface OwnedFile {
    file: FileOwners;
    names: readonly string[];
}

/** One run of changed lines: `count` added lines from `start`, or lines taken out just before `start` when `count` is 0. */
export interface ChangeBlock {
    start: number;
    count: number;
}

/** Who wrote a checkout's uncommitted lines, or its staged ones, read again whenever git reports a change. */
export function useChangeOwners(repo: string, staged: boolean, enabled = true): ChangeOwners | undefined {
    return useCachedResourceEnabled(enabled && !!repo, gitChangeOwnersR, repo, staged).data;
}

const ownedFiles = new WeakMap<ChangeOwners, Map<string, OwnedFile>>();

/** A file's owned lines, the same object for as long as `owners` is. */
export function ownedFile(owners: ChangeOwners | undefined, path: string): OwnedFile | undefined {
    if (!owners) return undefined;
    let files = ownedFiles.get(owners);
    if (!files) {
        const names = owners.authors.map((author) => author.name);
        files = new Map(owners.files.map((file) => [file.path, { file, names }]));
        ownedFiles.set(owners, files);
    }
    return files.get(path);
}

/** The lines of `path` a commit's note gives each of its agents, read as the uncommitted ones are; lines several share name them all. */
export function commitOwnedFile(owners: readonly CommitOwner[] | undefined, path: string): OwnedFile | undefined {
    if (!owners?.some((owner) => owner.lines?.[path])) return undefined;
    const writers = new Map<number, number[]>();
    owners.forEach((owner, index) => {
        for (const [first, last] of owner.lines?.[path] ?? []) {
            for (let line = first; line <= last; line++) {
                const at = writers.get(line);
                if (at) at.push(index);
                else writers.set(line, [index]);
            }
        }
    });
    const names: string[] = [];
    const nameIndex = new Map<string, number>();
    const lines: OwnedLines[] = [];
    for (const line of [...writers.keys()].sort((a, b) => a - b)) {
        const name = writers
            .get(line)!
            .map((index) => owners[index].name)
            .join(" or ");
        let author = nameIndex.get(name);
        if (author === undefined) {
            author = names.push(name) - 1;
            nameIndex.set(name, author);
        }
        const run = lines[lines.length - 1];
        if (run && run.author === author && run.start + run.count === line) run.count++;
        else lines.push({ start: line, count: 1, author });
    }
    return { file: { path, lines, removed: [] }, names };
}

/** The agents that wrote some of a file's uncommitted changes, in the order they first appear. */
export function fileAuthors(owners: ChangeOwners | undefined, path: string): ChangeAuthor[] {
    const owned = ownedFile(owners, path);
    if (!owners || !owned) return [];
    const authors = new Set([...owned.file.lines.map((run) => run.author), ...owned.file.removed.map((gap) => gap.author)]);
    return [...authors].map((author) => owners.authors[author]);
}

function ownerAt(runs: readonly OwnedLines[], line: number): OwnedLines | undefined {
    let low = 0;
    let high = runs.length;
    while (low < high) {
        const mid = (low + high) >> 1;
        if (runs[mid].start + runs[mid].count <= line) low = mid + 1;
        else high = mid;
    }
    const run = runs[low];
    return run && run.start <= line ? run : undefined;
}

/**
 * The quiet notes to show beside changed lines: a block one agent wrote names it on its first line, a block several
 * wrote names each where its writer changes, and a line that rewrote another agent's says so. Keyed by line number;
 * `gaps` holds the notes for lines taken out just before a line.
 */
export function ownerNotes(blocks: readonly ChangeBlock[], owned: OwnedFile | undefined): { lines: Map<number, string>; gaps: Map<number, string> } {
    const lines = new Map<number, string>();
    const gaps = new Map<number, string>();
    if (!owned) return { lines, gaps };
    const { file, names } = owned;
    for (const block of blocks) {
        if (block.count === 0) {
            const gap = file.removed.find((removal) => removal.at === block.start);
            if (gap) gaps.set(block.start, names[gap.author]);
            continue;
        }
        const owners: (OwnedLines | undefined)[] = [];
        for (let line = block.start; line < block.start + block.count; line++) owners.push(ownerAt(file.lines, line));
        const writers = new Set(owners.map((run) => run?.author));
        if (writers.size === 1 && owners[0] === undefined) continue;
        const mixed = writers.size > 1;
        owners.forEach((run, index) => {
            const before = index > 0 ? owners[index - 1] : undefined;
            const parts: string[] = [];
            if (index === 0 || (mixed && run?.author !== before?.author)) parts.push(run ? names[run.author] : "unattributed");
            if (run?.over !== undefined && (run.over !== before?.over || run.author !== before?.author))
                parts.push(`changed ${names[run.over]}'s edit`);
            if (parts.length) lines.set(block.start + index, parts.join(" · "));
        });
    }
    return { lines, gaps };
}

/** The notes for a unified diff's rows, where added rows carry new line numbers and unchanged rows do too. */
export function diffRowNotes(rows: readonly DiffRow[], owned: OwnedFile | undefined): (string | undefined)[] | null {
    if (!owned) return null;
    const blocks: ChangeBlock[] = [];
    const firstRows: number[] = [];
    let lastLine = 0;
    let open: ChangeBlock | null = null;
    rows.forEach((row, index) => {
        const [kind] = row;
        if (kind === DIFF_ROW.added || kind === DIFF_ROW.deleted) {
            if (!open) {
                open = { start: lastLine + 1, count: 0 };
                blocks.push(open);
                firstRows.push(index);
            }
            if (kind === DIFF_ROW.added) {
                if (open.count === 0) open.start = row[1];
                open.count++;
                lastLine = row[1];
            }
            return;
        }
        open = null;
        if (kind === DIFF_ROW.context) lastLine = row[1];
    });
    const { lines, gaps } = ownerNotes(blocks, owned);
    if (!lines.size && !gaps.size) return null;
    const notes: (string | undefined)[] = rows.map((row) => (row[0] === DIFF_ROW.added ? lines.get(row[1]) : undefined));
    blocks.forEach((block, index) => {
        if (block.count === 0) notes[firstRows[index]] = gaps.get(block.start);
    });
    return notes;
}
