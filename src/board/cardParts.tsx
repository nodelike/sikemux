import { AgentMark } from "../agents/AgentMark";
import { agentHandle } from "../agents/AgentLabel";
import { AgentStateIndicator } from "../agents/AgentStateIndicator";
import { agentDisplay } from "../state/agentStatus";
import { useStore } from "../state/store";

/** An agent as a card names it: its mark, its name and what it is doing now. */
export function Owner({ id, name, cli, paidBy }: { id: string; name: string | null; cli: string | null; paidBy?: string }) {
    const agent = useStore((s) => s.agents[id]);
    const state = useStore((s) => s.agentActivity[id]?.state);
    const display = agent && agent.launchState !== "dormant" ? agentDisplay(state) : null;
    return (
        <span className="board-owner">
            <AgentMark cli={agent?.type ?? cli} />
            <b>{agent?.name ? agentHandle(agent.name) : name ? agentHandle(name) : "An agent"}</b>
            {paidBy && (
                <span className="board-owner-paid" title={`Paid with ${paidBy}`}>
                    {paidBy}
                </span>
            )}
            {display && <AgentStateIndicator display={display} />}
        </span>
    );
}
