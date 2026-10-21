import { useMemo, type CSSProperties } from "react";
import type { CommitOwner, GitCommit } from "../api/git";
import { AgentLabel } from "../agents/AgentLabel";
import { AgentMark } from "../agents/AgentMark";
import { isAgentType } from "../agents/catalog";
import * as cmd from "../state/commands";
import { useCachedResourceEnabled } from "../state/resources";
import { gitCommitOwnersR } from "../state/resources.defs";
import { getState } from "../state/store";
import { agentForChat } from "../state/selectors";

export type CommitOwners = Record<string, CommitOwner[]>;

/** The agents each commit's footer names, for the commits that have one; asked of the core once for all of them. */
export function useCommitOwners(repo: string, commits: readonly GitCommit[]): CommitOwners | undefined {
    const footered = useMemo(
        () =>
            commits
                .filter((commit) => commit.footer)
                .map((commit) => commit.full_hash)
                .join(","),
        [commits],
    );
    return useCachedResourceEnabled(!!repo && footered !== "", gitCommitOwnersR, repo, footered).data;
}

/** Brings forward the agent that has the owner's session open, or resumes the session in a tab behind the one in front. */
function openOwner(owner: CommitOwner, repo: string): void {
    const { agents } = getState();
    const shown =
        (owner.open && agents[owner.open] ? owner.open : undefined) ??
        (owner.sessionId ? agentForChat(Object.values(agents), { agent: owner.cli, id: owner.sessionId }) : undefined)?.id;
    if (shown) return cmd.revealAgent(shown);
    if (!owner.sessionId || !isAgentType(owner.cli)) return;
    cmd.openSavedChat(
        { agent: owner.cli, id: owner.sessionId, title: owner.title ?? owner.name, project: owner.project ?? repo },
        { background: true },
    );
}

function describe(owner: CommitOwner): string {
    const who = owner.title ? `${owner.title} — ${owner.name}` : owner.name;
    const reach = owner.open ? "open now" : owner.sessionId ? "click to resume" : "its session is gone";
    const ran = [owner.cliVersion ? `${owner.cli} ${owner.cliVersion}` : owner.cli, owner.model, owner.sikemux && `Sikemux ${owner.sikemux}`];
    return `${who} (${ran.filter(Boolean).join(", ")}) wrote this · ${reach}`;
}

/** The agents a commit's signed footer names, each opening its session. `compact` shows only their marks when there are several. */
export function CommitAgents({ owners, repo, compact = false }: { owners: CommitOwner[] | undefined; repo: string; compact?: boolean }) {
    if (!owners?.length) return null;
    const marksOnly = compact && owners.length > 1;
    return (
        <span className="git-agents" style={{ "--agents": owners.length } as CSSProperties}>
            {owners.map((owner) => (
                <button
                    key={owner.chat}
                    type="button"
                    tabIndex={-1}
                    className={`git-agent${marksOnly ? " mark-only" : ""}`}
                    title={describe(owner)}
                    aria-label={describe(owner)}
                    disabled={!owner.open && !owner.sessionId}
                    onClick={(event) => {
                        event.stopPropagation();
                        openOwner(owner, repo);
                    }}
                    onDoubleClick={(event) => event.stopPropagation()}>
                    <AgentMark agentId={owner.chat} cli={owner.cli} size={11} />
                    {!marksOnly && <AgentLabel agent={{ title: owner.title ?? owner.name, name: owner.title ? owner.name : undefined }} />}
                </button>
            ))}
        </span>
    );
}
