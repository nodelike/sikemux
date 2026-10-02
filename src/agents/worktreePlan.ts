import { basename, dirname, joinPath, relativePath } from "../lib/paths";
import type { AgentWorktree } from "../state/types";

export const WORKTREE_BRANCH_PREFIX = "sikemux/";
const SLUG_MAX = 40;
const FALLBACK_SLUG = "chat";

/** A few words of the first message, kebab-cased, to name the branch and folder after. */
export function branchSlug(message: string): string {
    const words = message
        .replace(/https?:\/\/\S+/g, " ")
        .normalize("NFKD")
        .replace(/[̀-ͯ]/g, "")
        .toLowerCase()
        .split(/[^a-z0-9]+/)
        .filter(Boolean);
    let slug = "";
    for (const word of words) {
        const next = slug ? `${slug}-${word}` : word;
        if (next.length > SLUG_MAX) break;
        slug = next;
    }
    return slug || words[0]?.slice(0, SLUG_MAX) || FALLBACK_SLUG;
}

/** Beside the repository rather than in it, so the checkout never shows up in its own file tree or status. */
export function worktreesFolder(mainCheckout: string): string {
    return joinPath(dirname(mainCheckout), `${basename(mainCheckout)}.worktrees`);
}

export interface WorktreeTarget {
    branch: string;
    path: string;
}

export function worktreeCandidates(mainCheckout: string, slug: string, count = 20): WorktreeTarget[] {
    const folder = worktreesFolder(mainCheckout);
    return Array.from({ length: count }, (_, index) => {
        const name = index === 0 ? slug : `${slug}-${index + 1}`;
        return { branch: `${WORKTREE_BRANCH_PREFIX}${name}`, path: joinPath(folder, name) };
    });
}

export function pickWorktreeTarget(
    candidates: readonly WorktreeTarget[],
    takenBranches: ReadonlySet<string>,
    takenPaths: ReadonlySet<string>,
): WorktreeTarget | null {
    return candidates.find((candidate) => !takenBranches.has(candidate.branch) && !takenPaths.has(candidate.path)) ?? null;
}

/** The same folder in the new checkout that the project had open in the old one. */
export function cwdInWorktree(worktreePath: string, projectCwd: string, checkoutRoot: string): string {
    const inside = relativePath(projectCwd, checkoutRoot);
    return inside ? joinPath(worktreePath, inside) : worktreePath;
}

/** The checkout an agent's folder lies in, back in the main repository. */
export function cwdOutsideWorktree(worktree: AgentWorktree, agentCwd: string): string {
    const inside = relativePath(agentCwd, worktree.path);
    return inside ? joinPath(worktree.repo, inside) : worktree.repo;
}

/** A branch counts as merged once it has moved on from where it started and its base holds every commit on it. */
export function branchMergedLocally(tip: string | null, startSha: string, unmergedCommits: number): boolean {
    return !!tip && tip !== startSha && unmergedCommits === 0;
}
