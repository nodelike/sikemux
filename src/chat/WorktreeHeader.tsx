import { useEffect } from "react";
import { git } from "../api/git";
import { branchMergedLocally } from "../agents/worktreePlan";
import { pullMerged, useBranchPull, useHostRepo } from "../codehost/project";
import { subscribe } from "../state/bus";
import * as cmd from "../state/commands";
import { invalidate, resource, useResourceEnabled } from "../state/resources";
import type { AgentWorktree } from "../state/types";
import { IconPullRequest } from "../ui/Icons";
import "../styles/chat/worktree.css";

const PULL_STATE_LABEL: Record<string, string> = { open: "Open", draft: "Draft", merged: "Merged", closed: "Closed" };
const RECHECK_AFTER_CHANGE_MS = 1_500;

const worktreeMergedR = resource({
    kind: "agent.worktreeMerged",
    fetch: async (repo: string, base: string, branch: string, startSha: string): Promise<boolean> => {
        const [checkouts, compare] = await Promise.all([git.worktrees(repo), git.compare(repo, base, branch)]);
        const tip = checkouts.find((checkout) => checkout.branch === branch)?.head ?? null;
        return branchMergedLocally(tip, startSha, compare.commits.length);
    },
    staleAfterMs: 15_000,
});

/** Merging happens in a terminal as often as in the app, so a change to the repository is what asks again. */
function useRecheckOnChange(worktree: AgentWorktree, enabled: boolean): void {
    useEffect(() => {
        if (!enabled) return;
        let timer: number | undefined;
        const recheck = (event: { repo: string }) => {
            if (event.repo !== worktree.repo && event.repo !== worktree.path) return;
            window.clearTimeout(timer);
            timer = window.setTimeout(() => invalidate((kind) => kind === "agent.worktreeMerged"), RECHECK_AFTER_CHANGE_MS);
        };
        const stops = [subscribe("fs-changed", recheck), subscribe("git-refresh", recheck)];
        return () => {
            window.clearTimeout(timer);
            stops.forEach((stop) => stop());
        };
    }, [enabled, worktree.repo, worktree.path]);
}

/** The agent header's word on its worktree branch: the pull request it has, and a way out once that work is merged. */
export default function WorktreeHeader({ agentId, worktree, visible }: { agentId: string; worktree: AgentWorktree; visible: boolean }) {
    const host = useHostRepo(worktree.path, visible);
    const pull = useBranchPull(host.repo, worktree.branch, visible);
    const local = useResourceEnabled(
        visible && !!worktree.base,
        worktreeMergedR,
        worktree.repo,
        worktree.base ?? "",
        worktree.branch,
        worktree.startSha,
    );
    useRecheckOnChange(worktree, visible && !!worktree.base);
    const prMerged = pullMerged(pull);
    const merged = prMerged || local.data === true;
    const state = prMerged ? "merged" : pull?.draft && pull.state === "open" ? "draft" : (pull?.state ?? "");
    return (
        <>
            {pull && (
                <button
                    type="button"
                    className="agent-pull-badge"
                    data-state={state}
                    title={`#${pull.number} ${pull.title}`}
                    onClick={() => cmd.openUrlOnDesk(agentId, pull.url)}>
                    <IconPullRequest size={12} />
                    <span>#{pull.number}</span>
                    <span>{PULL_STATE_LABEL[state] ?? state}</span>
                </button>
            )}
            {merged && (
                <button
                    type="button"
                    className="agent-worktree-remove"
                    title={`${worktree.branch} is merged. Remove its worktree at ${worktree.path}.`}
                    onClick={() => void import("../agents/agentWorktree").then(({ removeAgentWorktree }) => removeAgentWorktree(agentId, prMerged))}>
                    Remove worktree
                </button>
            )}
        </>
    );
}
