import { agentHandle } from "../agents/AgentLabel";
import { AgentMark } from "../agents/AgentMark";
import { AgentStateIndicator, showsAgentState } from "../agents/AgentStateIndicator";
import { isAgentType } from "@shared/agents/catalog";
import type { RemoteAgent } from "../api/inbox";
import { IconAgent } from "@shared/ui/Icons";
import type { TeamLink, TeamMember } from "./teams";

/** A worker an agent here started on a linked host, as a row of its lead's team. */
export interface RemoteWorker extends TeamMember {
    readonly remote: RemoteAgent;
}

export function isRemoteWorker(row: TeamMember): row is RemoteWorker {
    return "remote" in row;
}

/** A lead's row heads its team's group, and its workers hang under it on the group's spine. */
export function teamWrapClass(link: TeamLink | undefined, open = true): string {
    return `agent-row-wrap${link === "head" ? ` team-head${open ? " open" : ""}` : link ? ` team-member${link === "last" ? " team-last" : ""}` : ""}`;
}

/** A worker on a linked host, in its lead's team. It has no chat here to open. */
export function RemoteAgentRow({ agent, link }: { agent: RemoteAgent; link?: TeamLink }) {
    return (
        <div className={teamWrapClass(link)}>
            <div className="agent-row remote" title={agent.reachable ? `Runs on ${agent.host}` : `${agent.host} is out of reach`}>
                <span className={`agent-glyph ${agent.cli}`}>
                    {isAgentType(agent.cli) ? <AgentMark agentId={agent.agentId} cli={agent.cli} size={20} /> : <IconAgent size={20} />}
                </span>
                <span className="agent-title">
                    <span className="agent-name">{agentHandle(agent.name)}</span>{" "}
                    <span className="agent-name-title">
                        on {agent.host}
                        {!agent.reachable && ", out of reach"}
                    </span>
                </span>
            </div>
            {agent.reachable && showsAgentState(agent) && (
                <span className="row-status">
                    <AgentStateIndicator display={agent} />
                </span>
            )}
        </div>
    );
}
