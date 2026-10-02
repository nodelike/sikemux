import { useMemo } from "react";
import { AgentStateIndicator } from "../agents/AgentStateIndicator";
import * as cmd from "../state/commands";
import { useStore } from "../state/store";
import { activeAgentId, agentIdsOf } from "../state/selectors";
import type { Agent, AgentPresentationState } from "../state/types";
import { AgentIcon, IconClose } from "../ui/Icons";
import { Panel, PanelHeader } from "../ui/Panel";
import { Tooltip } from "../ui/Tooltip";

type Bucket = "blocked" | "working" | "done" | "idle";

const BUCKETS: { id: Bucket; label: string }[] = [
    { id: "blocked", label: "Needs you" },
    { id: "working", label: "Working" },
    { id: "done", label: "Done" },
    { id: "idle", label: "Idle" },
];

interface OpenAgent {
    agent: Agent;
    state: AgentPresentationState;
    bucket: Bucket;
    sessionName: string;
}

/** Every agent open in any project, grouped by what it needs from you. */
export function AllProjectsAgents() {
    const sessionOrder = useStore((s) => s.sessionOrder);
    const sessions = useStore((s) => s.sessions);
    const windows = useStore((s) => s.windows);
    const windowsBySession = useStore((s) => s.windowsBySession);
    const agents = useStore((s) => s.agents);
    const activity = useStore((s) => s.agentActivity);
    const background = useStore((s) => s.agentBackgroundWork);
    const shownAgentId = useStore((s) => activeAgentId(s, s.sessions[s.activeSessionId]));

    const open = useMemo(() => {
        const rows: OpenAgent[] = [];
        for (const sessionId of sessionOrder) {
            const session = sessions[sessionId];
            if (session?.kind !== "project") continue;
            for (const id of agentIdsOf({ windows, windowsBySession }, sessionId)) {
                const agent = agents[id];
                if (!agent) continue;
                const state = activity[id]?.state ?? "idle";
                const working = state === "working" || (background[id] ?? 0) > 0;
                const bucket: Bucket = state === "blocked" ? "blocked" : working ? "working" : state === "done" ? "done" : "idle";
                rows.push({ agent, state: working && state !== "blocked" ? "working" : state, bucket, sessionName: session.name });
            }
        }
        return rows;
    }, [sessionOrder, sessions, windows, windowsBySession, agents, activity, background]);

    if (open.length === 0) return <div className="agent-empty">no agents open in any project</div>;

    return (
        <>
            {BUCKETS.map(({ id, label }) => {
                const rows = open.filter((row) => row.bucket === id);
                if (rows.length === 0) return null;
                return (
                    <Panel key={id} variant="group" className={`agent-group${id === "blocked" ? " agent-attention" : ""}`}>
                        <PanelHeader label={`${label} ${rows.length}`} rule />
                        {rows.map(({ agent, state, bucket, sessionName }) => (
                            <div key={agent.id} className="agent-row-wrap">
                                <button
                                    className={`agent-row${agent.id === shownAgentId ? " active" : ""}`}
                                    title={`${sessionName} — ${agent.title}`}
                                    onClick={() => cmd.revealAgent(agent.id)}>
                                    <span className={`agent-glyph ${agent.type}`}>
                                        <AgentIcon type={agent.type} size={20} />
                                    </span>
                                    <span className="agent-title">{agent.title}</span>
                                </button>
                                {bucket !== "idle" && (
                                    <span className="row-status">
                                        <AgentStateIndicator state={state} />
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
