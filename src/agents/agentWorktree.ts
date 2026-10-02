import { fsapi } from "../api/fs";
import { git } from "../api/git";
import { worktreeHasLiveOwners } from "../git/worktreeLifecycle";
import { loadProjectConfig } from "../projects/projectConfig";
import { trustProjectConfig, worktreeHookCommand } from "../projects/projectConfigRuntime";
import { projectControllerBridge } from "../projects/controllerBridge";
import { emit } from "../state/bus";
import * as cmd from "../state/commands";
import { runBackgroundCommand } from "../state/commands/customCommands";
import { confirmDialog } from "../state/dialog";
import { invalidate } from "../state/resources";
import { agentWindowId, ownerSessionId } from "../state/selectors";
import { getState } from "../state/store";
import { errMessage, notify } from "../state/toast";
import type { AgentWorktree } from "../state/types";
import { branchSlug, cwdInWorktree, cwdOutsideWorktree, pickWorktreeTarget, worktreeCandidates } from "./worktreePlan";

export interface PreparedWorktree {
    worktree: AgentWorktree;
    /** Where the agent works inside the new checkout. */
    cwd: string;
    /** Why the project's setup commands did not all finish, when they did not. */
    setupError: string | null;
}

function refreshWorktrees(repo: string): void {
    invalidate((kind) => kind === "git.worktrees" || kind === "agent.worktreeMerged");
    void projectControllerBridge.refresh(repo);
    emit({ type: "git-refresh", repo });
}

/** Cuts a new branch off the project's HEAD into a checkout beside the repository, then runs the project's worktree setup in it. */
export async function createAgentWorktree({
    agentId,
    projectCwd,
    message,
    onStep,
}: {
    agentId: string;
    projectCwd: string;
    message: string;
    onStep: (step: string) => void;
}): Promise<PreparedWorktree> {
    onStep("Creating worktree");
    const [worktrees, branches] = await Promise.all([git.worktrees(projectCwd), git.branches(projectCwd)]);
    const main = worktrees.find((worktree) => worktree.is_main);
    const current = worktrees.find((worktree) => worktree.current);
    if (!main || !current?.head) throw new Error("The project has no commit to branch from");
    const candidates = worktreeCandidates(main.path, branchSlug(message));
    const kinds = await fsapi.pathKinds(candidates.map((candidate) => candidate.path));
    const takenPaths = new Set([
        ...worktrees.map((worktree) => worktree.path),
        ...candidates.filter((_, index) => kinds[index] !== null).map((candidate) => candidate.path),
    ]);
    const target = pickWorktreeTarget(candidates, new Set(branches.map((branch) => branch.name)), takenPaths);
    if (!target) throw new Error("Every worktree name for this message is already taken");
    const created = await git.worktreeCreate(projectCwd, { path: target.path, branch: target.branch, createBranch: true, startPoint: current.head });
    const worktree: AgentWorktree = { repo: main.path, path: created.path, branch: target.branch, base: current.branch, startSha: current.head };
    refreshWorktrees(main.path);
    const cwd = cwdInWorktree(created.path, projectCwd, current.path);
    const setupError = await runWorktreeSetup(agentId, projectCwd, cwd, onStep);
    return { worktree, cwd, setupError };
}

async function runWorktreeSetup(agentId: string, projectCwd: string, cwd: string, onStep: (step: string) => void): Promise<string | null> {
    const config = await loadProjectConfig(projectCwd);
    if (config.status === "absent") return null;
    if (config.status === "invalid")
        return `sikemux.json needs attention, so the worktree setup did not run: ${config.errors[0]?.message ?? "invalid"}`;
    const hooks = config.config.worktree?.onCreate ?? [];
    if (hooks.length === 0) return null;
    onStep("Waiting for sikemux.json to be trusted");
    if (!(await trustProjectConfig(config))) return "The worktree setup in sikemux.json was not trusted, so it did not run";
    const state = getState();
    const windowId = agentWindowId(state, agentId);
    const sessionId = (windowId && ownerSessionId(state, windowId)) || undefined;
    for (const hook of hooks) {
        onStep(`Running ${hook.label}`);
        try {
            await runBackgroundCommand(worktreeHookCommand(hook), cwd, true, sessionId);
        } catch (failure) {
            return errMessage(failure);
        }
    }
    return null;
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
    if (state.agentActivity[agentId]?.backendState === "working") {
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
        notify("error", `Could not remove worktree ${worktree.branch}: ${errMessage(failure)}`);
    } finally {
        refreshWorktrees(worktree.repo);
    }
}
