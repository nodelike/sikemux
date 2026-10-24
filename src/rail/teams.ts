import { useCallback, useState } from "react";
import type { AgentShown, RemoteAgent, TeamLink } from "../api/generated/wire";
import type { RemoteWorker } from "./RemoteAgentRow";
import { inCoreOrder } from "./useInbox";

export type { TeamLink };

export interface TeamMember {
    readonly id: string;
}

export interface TeamRow<T extends TeamMember> {
    agent: T;
    link?: TeamLink;
}

const inRun = (link: TeamLink | undefined) => link === "member" || link === "last";

/**
 * The rows in the order the core lists its agents, on the teams it links them into, with the workers each team
 * started on linked hosts after its last agent here, as the core places them. A lead whose team is not among the
 * rows stands on its own.
 */
export function coreTeams<T extends TeamMember>(
    rows: readonly T[],
    shown: readonly AgentShown[],
    remote: readonly RemoteAgent[],
): TeamRow<T | RemoteWorker>[] {
    const linkOf = new Map(shown.map((agent) => [agent.agentId, agent.team]));
    const local = inCoreOrder(rows, shown).map((agent): TeamRow<T | RemoteWorker> => ({ agent, link: linkOf.get(agent.id) }));
    const placed = new Set<string>();
    const listed: TeamRow<T | RemoteWorker>[] = [];
    for (let at = 0; at < local.length;) {
        let end = at;
        if (local[at].link === "head") while (end + 1 < local.length && inRun(local[end + 1].link)) end += 1;
        const team = local.slice(at, end + 1);
        const ids = new Set(team.map((row) => row.agent.id));
        const workers = remote.filter((agent) => agent.lead && ids.has(agent.lead) && !placed.has(agent.agentId));
        workers.forEach((agent) => placed.add(agent.agentId));
        listed.push(...team, ...workers.map((agent) => ({ agent: { id: agent.agentId, remote: agent }, link: agent.team })));
        at = end + 1;
    }
    return listed.map((row, index) => (row.link === "head" && !inRun(listed[index + 1]?.link) ? { agent: row.agent } : row));
}

/** How a lead's row folds its team: whether the workers show, and how many there are. */
export interface TeamFold {
    open: boolean;
    count: number;
    toggle: () => void;
}

export type FoldedRow<T extends TeamMember> = TeamRow<T> & { fold?: TeamFold };

/** Which leads have their teams folded away, for one list of agents. */
export function useTeamFolds(): { folded: ReadonlySet<string>; toggle: (lead: string) => void } {
    const [folded, setFolded] = useState<ReadonlySet<string>>(new Set());
    const toggle = useCallback(
        (lead: string) =>
            setFolded((now) => {
                const next = new Set(now);
                if (!next.delete(lead)) next.add(lead);
                return next;
            }),
        [],
    );
    return { folded, toggle };
}

/** The rows with each lead's fold on it, and a folded lead's workers left out. */
export function foldTeams<T extends TeamMember>(
    rows: readonly TeamRow<T>[],
    folded: ReadonlySet<string>,
    toggle: (lead: string) => void,
): FoldedRow<T>[] {
    const shown: FoldedRow<T>[] = [];
    let lead: string | null = null;
    rows.forEach((row, index) => {
        if (row.link === "head") {
            lead = row.agent.id;
            const id = lead;
            let count = 0;
            for (let next = index + 1; next < rows.length && (rows[next].link === "member" || rows[next].link === "last"); next++) count++;
            shown.push({ ...row, fold: { open: !folded.has(id), count, toggle: () => toggle(id) } });
        } else if (row.link === "member" || row.link === "last") {
            if (!lead || !folded.has(lead)) shown.push(row);
        } else {
            lead = null;
            shown.push(row);
        }
    });
    return shown;
}
