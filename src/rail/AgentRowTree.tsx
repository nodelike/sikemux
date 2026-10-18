import { useState, type ReactNode } from "react";
import { AgentReview } from "./AgentReview";
import { useBranchPullList, useHostRepo } from "../codehost/project";
import * as cmd from "../state/commands";
import type { AgentWorktree } from "../state/types";
import { IconFolder, IconGit, IconPullRequest } from "../ui/Icons";

/** The project a row's agent works in, on a second line under its title when the rail shows every project. */
export function AgentRowProject({ name }: { name: string }) {
    return (
        <span className="agent-row-project">
            <IconFolder size={11} />
            <span>{name}</span>
        </span>
    );
}

/**
 * What an agent row is tied to, hung off the row as the side rail hangs a project's views:
 * branch, pull requests, then tickets and thread. Each name opens what it names, and
 * several of one kind share a line, split by dots.
 *
 * Not built yet, waiting on agents recording them:
 * - Jira tickets: one item with the Jira mark, ids side by side, each opening its issue on the desk.
 * - Slack thread: one item with the Slack mark, `#channel · thread name`, each opening the thread.
 */
export function AgentRowTree({
    agentId,
    worktree,
    runsOn = false,
}: {
    agentId?: string;
    worktree?: AgentWorktree;
    /** The tree's spine goes on past its last item, as the line of a team that continues below. */
    runsOn?: boolean;
}) {
    const [reviewAt, setReviewAt] = useState<DOMRect | null>(null);
    if (!worktree) return null;
    return (
        <div className={`proj-children agent-tree${runsOn ? " runs-on" : ""}`}>
            <TreeItem icon={<IconGit size={13} />}>
                {agentId ? (
                    <button
                        type="button"
                        className="agent-tree-link agent-tree-mono"
                        title={`Review ${worktree.branch}`}
                        onClick={(event) => {
                            event.stopPropagation();
                            setReviewAt(event.currentTarget.getBoundingClientRect());
                        }}>
                        {worktree.branch}
                    </button>
                ) : (
                    <span className="agent-tree-mono" title={worktree.path}>
                        {worktree.branch}
                    </span>
                )}
            </TreeItem>
            {agentId && reviewAt && <AgentReview agentId={agentId} worktree={worktree} anchor={reviewAt} onClose={() => setReviewAt(null)} />}
            <WorktreePulls agentId={agentId} worktree={worktree} />
        </div>
    );
}

function TreeItem({ icon, className, children }: { icon: ReactNode; className?: string; children: ReactNode }) {
    return (
        <div className={`proj-child${className ? ` ${className}` : ""}`}>
            <span className="proj-child-tick" />
            <span className="proj-child-ic">{icon}</span>
            <span className="proj-child-label agent-tree-items">{children}</span>
        </div>
    );
}

function WorktreePulls({ agentId, worktree }: { agentId?: string; worktree: AgentWorktree }) {
    const host = useHostRepo(worktree.path, true);
    const pulls = useBranchPullList(host.repo, worktree.branch, true);
    if (pulls.length === 0) return null;
    return (
        <TreeItem icon={<IconPullRequest size={13} />}>
            {pulls.map((pull) => (
                <button
                    key={pull.number}
                    type="button"
                    className="agent-tree-link agent-tree-mono"
                    title={pull.title}
                    onClick={() => (agentId ? cmd.openUrlOnDesk(agentId, pull.url) : undefined)}>
                    #{pull.number}
                </button>
            ))}
        </TreeItem>
    );
}
