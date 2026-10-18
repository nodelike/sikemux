import { agentHandle } from "../agents/AgentLabel";
import { AgentStateIndicator, showsAgentState } from "../agents/AgentStateIndicator";
import { isAgentType } from "../agents/catalog";
import type { TeamLink, TeamMember } from "../agents/teams";
import type { RemoteAgent, RemoteAgentStatus } from "../api/inbox";
import { agentDisplay } from "../state/agentStatus";
import type { AgentPresentationState } from "../state/types";
import { AgentIcon, IconAgent } from "../ui/Icons";

export interface RemoteWorker extends TeamMember {
    readonly remote: RemoteAgent;
}

/** The rows, then the workers their agents started on linked hosts, ready for `inTeams` to place under their leads. */
export function withRemoteWorkers<T extends TeamMember>(rows: readonly T[], remote: readonly RemoteAgent[]): (T | RemoteWorker)[] {
    const leads = new Set(rows.map((row) => row.id));
    const workers = remote
        .filter((agent) => agent.lead && leads.has(agent.lead))
        .map((agent) => ({ id: agent.agentId, lead: agent.lead, remote: agent }));
    return [...rows, ...workers];
}

export function isRemoteWorker(row: TeamMember): row is RemoteWorker {
    return "remote" in row;
}

export function teamWrapClass(link: TeamLink | undefined): string {
    return `agent-row-wrap${link === "head" ? " chain-head" : link ? ` chain${link === "last" ? " chain-last" : ""}` : ""}`;
}

function presentation(status: RemoteAgentStatus): AgentPresentationState {
    if (status === "starting") return "working";
    return status === "working" || status === "blocked" || status === "done" ? status : "idle";
}

/** A worker on a linked host, in its lead's team. It has no chat here to open. */
export function RemoteAgentRow({ agent, link }: { agent: RemoteAgent; link?: TeamLink }) {
    const display = agentDisplay(presentation(agent.status));
    return (
        <div className={teamWrapClass(link)}>
            <div className="agent-row remote" title={agent.reachable ? `Runs on ${agent.host}` : `${agent.host} is out of reach`}>
                <span className={`agent-glyph ${agent.cli}`}>
                    {isAgentType(agent.cli) ? <AgentIcon type={agent.cli} size={20} /> : <IconAgent size={20} />}
                </span>
                <span className="agent-title">
                    <span className="agent-name">{agentHandle(agent.name)}</span>{" "}
                    <span className="agent-name-title">
                        on {agent.host}
                        {!agent.reachable && ", out of reach"}
                    </span>
                </span>
            </div>
            {agent.reachable && showsAgentState(display) && (
                <span className="row-status">
                    <AgentStateIndicator display={display} />
                </span>
            )}
        </div>
    );
}
