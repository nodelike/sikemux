import { git } from "../api/git";
import { worktreeHasLiveOwners } from "../git/worktreeLifecycle";
import { emit } from "../state/bus";
import * as cmd from "../state/commands";
import { confirmDialog } from "../state/dialog";
import { invalidate } from "../state/resources";
import { getState } from "../state/store";
import { notify } from "../state/toast";
import { errorText } from "@shared/lib/format";
import type { AgentWorktree } from "../state/types";
import { cwdOutsideWorktree } from "./worktreePlan";

function refreshWorktrees(repo: string): void {
    invalidate((kind) => kind === "git.worktrees" || kind === "agent.worktreeMerged");
    emit({ type: "git-refresh", repo });
}

/** Commits on the branch that its base does not have yet. */
async function unmergedCommits(worktree: AgentWorktree): Promise<number> {
    const compare = await git.compare(worktree.repo, worktree.base ?? worktree.startSha, worktree.branch);
    return compare.commits.length;
}

/**
 * Takes the checkout away and moves the agent back into the main one. The
 * branch goes too once nothing on it would be lost; otherwise the person decides.
 */
export async function removeAgentWorktree(agentId: string, pullMerged = false): Promise<void> {
    const state = getState();
    const agent = state.agents[agentId];
    const worktree = agent?.worktree;
    if (!agent || !worktree) return;
    if (worktreeHasLiveOwners(state, worktree.path, agentId)) {
        notify("info", "Another agent or project still uses this worktree. Close it before removing the worktree.");
        return;
    }
    if (state.agentStatus[agentId]?.busy) {
        notify("info", "Stop the agent's turn before removing its worktree");
        return;
    }
    try {
        const status = await git.status(worktree.path);
        const dirty = status.files.length > 0;
        if (dirty) {
            const ok = await confirmDialog({
                title: `Remove worktree ${worktree.branch}?`,
                body: `${status.files.length} uncommitted change${status.files.length === 1 ? "" : "s"} in ${worktree.path} will be lost.`,
                confirmLabel: "Remove",
                destructive: true,
            });
            if (!ok) return;
        }
        const safeToDelete = pullMerged || (await unmergedCommits(worktree).catch(() => 1)) === 0;
        await git.worktreeRemove(worktree.repo, worktree.path, dirty);
        cmd.setAgentWorktree(agentId, cwdOutsideWorktree(worktree, agent.cwd ?? worktree.path), null);
        const deleteBranch =
            safeToDelete ||
            (await confirmDialog({
                title: `Delete branch ${worktree.branch} too?`,
                body: `It has commits that are not in ${worktree.base ?? "the branch it started from"}. Deleting it loses any that were not pushed.`,
                confirmLabel: "Delete branch",
                cancelLabel: "Keep branch",
                destructive: true,
            }));
        if (deleteBranch) await git.branchDelete(worktree.repo, worktree.branch, true);
        notify("success", deleteBranch ? `Removed worktree and branch ${worktree.branch}` : `Removed worktree ${worktree.branch}`);
    } catch (failure) {
        notify("error", `Could not remove worktree ${worktree.branch}: ${errorText(failure)}`);
    } finally {
        refreshWorktrees(worktree.repo);
    }
}
