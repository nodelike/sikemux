import { useMemo } from "react";
import { AgentStateIndicator, BackgroundCounts, showsAgentState } from "../agents/AgentStateIndicator";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { AGENT_GROUPS, AGENT_LABELS, lastWorkedAt, type AgentDisplay } from "../state/agentStatus";
import { activeAgentId, agentDisplayOf, openAgents } from "../state/selectors";
import type { Agent } from "../state/types";
import { AgentIcon, IconClose } from "../ui/Icons";
import { Panel, PanelHeader } from "../ui/Panel";
import { Tooltip } from "../ui/Tooltip";

interface OpenAgentRow {
    agent: Agent;
    display: AgentDisplay;
    sessionName: string;
}

/** Every agent open in any project, grouped by what it needs from you. */
export function AllProjectsAgents() {
    const sessionOrder = useStore((s) => s.sessionOrder);
    const sessions = useStore((s) => s.sessions);
    const windows = useStore((s) => s.windows);
    const windowsBySession = useStore((s) => s.windowsBySession);
    const agents = useStore((s) => s.agents);
    const agentActivity = useStore((s) => s.agentActivity);
    const agentTasks = useStore((s) => s.agentTasks);
    const agentSubagents = useStore((s) => s.agentSubagents);
    const shownAgentId = useStore((s) => activeAgentId(s, s.sessions[s.activeSessionId]));

    const open = useMemo(() => {
        const rows: OpenAgentRow[] = [];
        for (const { agentId, agent, session } of openAgents({ sessionOrder, sessions, windows, windowsBySession, agents }))
            rows.push({ agent, display: agentDisplayOf({ agentActivity, agentTasks, agentSubagents }, agentId), sessionName: session.name });
        return rows;
    }, [sessionOrder, sessions, windows, windowsBySession, agents, agentActivity, agentTasks, agentSubagents]);

    if (open.length === 0) return <div className="agent-empty">no agents open in any project</div>;

    return (
        <>
            {AGENT_GROUPS.map((id) => {
                const rows = open.filter((row) => row.display.group === id);
                if (id === "done" || id === "idle")
                    rows.sort((x, y) => lastWorkedAt(agentActivity[y.agent.id]) - lastWorkedAt(agentActivity[x.agent.id]));
                if (rows.length === 0) return null;
                return (
                    <Panel key={id} variant="group" className={`agent-group${id === "blocked" ? " agent-attention" : ""}`}>
                        <PanelHeader label={`${AGENT_LABELS[id]} ${rows.length}`} rule />
                        {rows.map(({ agent, display, sessionName }) => (
                            <div key={agent.id} className="agent-row-wrap">
                                <button
                                    className={`agent-row${agent.id === shownAgentId ? " active" : ""}`}
                                    title={`${sessionName} — ${agent.title}`}
                                    onClick={() => cmd.revealAgent(agent.id)}>
                                    <span className={`agent-glyph ${agent.type}`}>
                                        <AgentIcon type={agent.type} size={20} />
                                    </span>
                                    <span className="agent-title">{agent.title}</span>
                                    <BackgroundCounts display={display} />
                                </button>
                                {showsAgentState(display) && (
                                    <span className="row-status">
                                        <AgentStateIndicator display={display} />
                                    </span>
                                )}
                                <Tooltip label={`Close ${agent.title}`}>
                                    <button
                                        type="button"
                                        className="row-x"
                                        aria-label={`Close ${agent.title}`}
                                        onClick={() => cmd.closeAgent(agent.id)}>
                                        <IconClose size={11} />
                                    </button>
                                </Tooltip>
                            </div>
                        ))}
                    </Panel>
                );
            })}
        </>
    );
}
