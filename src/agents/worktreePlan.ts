import { joinPath, relativePath } from "@shared/lib/paths";
import type { AgentWorktree } from "../state/types";

/** The checkout an agent's folder lies in, back in the main repository. */
export function cwdOutsideWorktree(worktree: AgentWorktree, agentCwd: string): string {
    const inside = relativePath(agentCwd, worktree.path);
    return inside ? joinPath(worktree.repo, inside) : worktree.repo;
}

/** A branch counts as merged once it has moved on from where it started and its base holds every commit on it. */
export function branchMergedLocally(tip: string | null, startSha: string, unmergedCommits: number): boolean {
    return !!tip && tip !== startSha && unmergedCommits === 0;
}
