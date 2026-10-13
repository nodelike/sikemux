import type { GitFile } from "../api/git";

export interface GitStatusDecoration {
    letter: string;
    /** Which colour it takes, as the class `git-<cls>`: conflicted, untracked, added, deleted, renamed or modified. */
    cls: "c" | "u" | "a" | "d" | "r" | "m";
    label: string;
}

const MODIFIED: GitStatusDecoration = { letter: "M", cls: "m", label: "modified" };

const BY_CODE: Record<string, GitStatusDecoration> = {
    "?": { letter: "U", cls: "u", label: "untracked" },
    U: { letter: "U", cls: "c", label: "unmerged" },
    A: { letter: "A", cls: "a", label: "added" },
    D: { letter: "D", cls: "d", label: "deleted" },
    R: { letter: "R", cls: "r", label: "renamed" },
    C: { letter: "C", cls: "r", label: "copied" },
    T: { letter: "T", cls: "m", label: "type changed" },
    M: MODIFIED,
};

/** One side of git's two-letter status, such as `M` or `?`; null when that side has no change. */
export function gitStatusDecoration(raw: string): GitStatusDecoration | null {
    const code = raw.trim();
    if (!code) return null;
    return BY_CODE[code] ?? { letter: code, cls: "m", label: code };
}

/** A code host's word for a changed file, such as `added` or `removed`, in git's letters and colours. */
const BY_HOST_STATUS: Record<string, GitStatusDecoration> = {
    added: BY_CODE.A,
    removed: BY_CODE.D,
    renamed: BY_CODE.R,
    copied: BY_CODE.C,
};

export function hostFileStatus(status: string): GitStatusDecoration {
    return BY_HOST_STATUS[status] ?? MODIFIED;
}

const RANK: GitStatusDecoration["cls"][] = ["c", "u", "d", "a", "r", "m"];

function mostTelling(statuses: GitStatusDecoration[]): GitStatusDecoration | undefined {
    return RANK.map((cls) => statuses.find((status) => status.cls === cls)).find(Boolean) ?? statuses[0];
}

export function gitFileDecoration(file: GitFile): GitStatusDecoration {
    const statuses = [gitStatusDecoration(file.index), gitStatusDecoration(file.worktree)].filter(
        (status): status is GitStatusDecoration => status !== null,
    );
    return mostTelling(statuses) ?? MODIFIED;
}

/**
 * Every folder that holds a changed file, by its path from the repository root, with the most telling change inside it.
 * Git lists a wholly untracked folder as the folder itself, so that folder counts as well as the ones above it.
 */
export function gitFolderDecorations(files: GitFile[]): Map<string, GitStatusDecoration> {
    const found = new Map<string, GitStatusDecoration[]>();
    for (const file of files) {
        const decoration = gitFileDecoration(file);
        const parts = file.path.replaceAll("\\", "/").split("/").filter(Boolean);
        const lastFolder = file.path.endsWith("/") ? parts.length : parts.length - 1;
        for (let depth = 1; depth <= lastFolder; depth++) {
            const folder = parts.slice(0, depth).join("/");
            found.set(folder, [...(found.get(folder) ?? []), decoration]);
        }
    }
    return new Map([...found].map(([folder, statuses]) => [folder, mostTelling(statuses)!]));
}

export interface GitStatusBadge extends GitStatusDecoration {
    source: string;
}

export function gitStatusBadge(raw: string, source: string): GitStatusBadge | null {
    const decoration = gitStatusDecoration(raw);
    return decoration ? { ...decoration, source } : null;
}

// Git spells untracked as `??` and unmerged as `UU`. Both sides carry the same
// letter, so those are one state, not a staged/unstaged pair.
export function gitFileBadges(file: GitFile): GitStatusBadge[] {
    if (file.index === "U" || file.worktree === "U") return [{ ...BY_CODE.U, source: "unmerged" }];
    if (file.index === "?" && file.worktree === "?") return [{ ...BY_CODE["?"], source: "untracked" }];
    return [gitStatusBadge(file.index, "staged"), gitStatusBadge(file.worktree, "unstaged")].filter(
        (badge): badge is GitStatusBadge => badge !== null,
    );
}
