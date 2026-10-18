import type { Agent } from "../state/types";

/** An agent's name as people see it and @mention it. */
export function agentHandle(name: string): string {
    return `@${name}`;
}

/** An agent as its rail row and header show it: what it is working on, then its name, dimmed. */
export function AgentLabel({ agent }: { agent: Pick<Agent, "name" | "title"> }) {
    if (!agent.name) return <>{agent.title}</>;
    return (
        <>
            <span className="agent-label-title">{agent.title}</span>
            <span className="agent-label-name">{agentHandle(agent.name)}</span>
        </>
    );
}
