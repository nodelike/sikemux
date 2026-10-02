import type { GitFile } from "../api/git";

export interface GitStatusDecoration {
    letter: string;
    cls: "m" | "u" | "a" | "d" | "r";
    label: string;
}

export function gitStatusDecoration(raw: string): GitStatusDecoration | null {
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

export function gitFileDecoration(file: GitFile): GitStatusDecoration {
    const index = gitStatusDecoration(file.index);
    const worktree = gitStatusDecoration(file.worktree);
    const statuses = [index, worktree].filter((status): status is GitStatusDecoration => status !== null);
    return (
        statuses.find((status) => status.cls === "u") ??
        statuses.find((status) => status.cls === "d") ??
        statuses.find((status) => status.cls === "a") ??
        statuses.find((status) => status.cls === "r") ??
        statuses[0] ?? { letter: "M", cls: "m", label: "modified" }
    );
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
