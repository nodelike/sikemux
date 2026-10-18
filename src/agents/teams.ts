/** Where a row sits on its lead's team line: the lead heads it, the last worker ends it. */
export type TeamLink = "head" | "member" | "last";

export interface TeamMember {
    readonly id: string;
    /** The agent that started this one. */
    readonly lead?: string | null;
}

export interface TeamRow<T extends TeamMember> {
    agent: T;
    link?: TeamLink;
}

/**
 * The rows in the order given, but each agent's workers right after it, in the same order, and theirs after them.
 * A worker whose lead is not among the rows stands on its own.
 */
export function inTeams<T extends TeamMember>(agents: readonly T[]): TeamRow<T>[] {
    const present = new Set(agents.map((agent) => agent.id));
    const follows = (agent: T): agent is T & { lead: string } => !!agent.lead && present.has(agent.lead) && agent.lead !== agent.id;
    const workers = new Map<string, T[]>();
    for (const agent of agents) {
        if (follows(agent)) workers.set(agent.lead, [...(workers.get(agent.lead) ?? []), agent]);
    }
    const placed = new Set<string>();
    const team = (lead: T): T[] => {
        placed.add(lead.id);
        return [lead, ...(workers.get(lead.id) ?? []).filter((worker) => !placed.has(worker.id)).flatMap(team)];
    };
    const rows: TeamRow<T>[] = [];
    for (const agent of agents) {
        if (placed.has(agent.id) || follows(agent)) continue;
        const members = team(agent);
        members.forEach((member, index) => {
            const link: TeamLink | undefined =
                members.length === 1 ? undefined : index === 0 ? "head" : index === members.length - 1 ? "last" : "member";
            rows.push({ agent: member, link });
        });
    }
    for (const agent of agents) if (!placed.has(agent.id)) rows.push({ agent });
    return rows;
}
