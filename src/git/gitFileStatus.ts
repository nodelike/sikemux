import type { GitFile } from "../api/git";

export interface GitStatusDecoration {
    letter: string;
    cls: "m" | "u" | "a" | "d" | "r";
    label: string;
}

function gitStatusDecoration(raw: string): GitStatusDecoration | null {
    const code = raw.trim();
    if (!code) return null;
    if (code === "?" || code === "U") return { letter: "U", cls: "u", label: code === "?" ? "untracked" : "unmerged" };
    if (code === "A") return { letter: "A", cls: "a", label: "added" };
    if (code === "D") return { letter: "D", cls: "d", label: "deleted" };
    if (code === "R") return { letter: "R", cls: "r", label: "renamed" };
    if (code === "C") return { letter: "C", cls: "r", label: "copied" };
    if (code === "T") return { letter: "T", cls: "m", label: "type changed" };
    return { letter: code, cls: "m", label: code === "M" ? "modified" : code };
}

const RANK: GitStatusDecoration["cls"][] = ["u", "d", "a", "r", "m"];

function mostTelling(statuses: GitStatusDecoration[]): GitStatusDecoration | undefined {
    return RANK.map((cls) => statuses.find((status) => status.cls === cls)).find(Boolean) ?? statuses[0];
}

export function gitFileDecoration(file: GitFile): GitStatusDecoration {
    const statuses = [gitStatusDecoration(file.index), gitStatusDecoration(file.worktree)].filter(
        (status): status is GitStatusDecoration => status !== null,
    );
    return mostTelling(statuses) ?? { letter: "M", cls: "m", label: "modified" };
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
    if (file.index === "U" || file.worktree === "U") return [{ letter: "U", cls: "u", label: "unmerged", source: "unmerged" }];
    if (file.index === "?" && file.worktree === "?") return [{ letter: "U", cls: "u", label: "untracked", source: "untracked" }];
    return [gitStatusBadge(file.index, "staged"), gitStatusBadge(file.worktree, "unstaged")].filter(
        (badge): badge is GitStatusBadge => badge !== null,
    );
}
