import { AgentMark } from "../agents/AgentMark";
import { agentHandle } from "../agents/AgentLabel";
import { AgentStateIndicator } from "../agents/AgentStateIndicator";
import { useStore } from "../state/store";

/** An agent as a card names it: its mark, its name and what it is doing now. */
export function Owner({ id, name, cli, paidBy }: { id: string; name: string | null; cli: string | null; paidBy: string | null }) {
    const agent = useStore((s) => s.agents[id]);
    const shown = useStore((s) => s.agentStatus[id]);
    const display = agent && agent.launchState !== "dormant" ? shown : undefined;
    return (
        <span className="board-owner">
            <AgentMark agentId={id} cli={cli} />
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
