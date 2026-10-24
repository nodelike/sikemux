import { useMemo } from "react";
import { AgentBuddy } from "../buddies/Buddy";
import { useStore } from "../state/store";
import { AgentIcon } from "../ui/Icons";
import { isAgentType } from "./catalog";

/** An agent's buddy when buddies are on and the agent is known, otherwise the mark of the CLI it runs, or nothing for one Sikemux does not know. */
export function AgentMark({ agentId, cli, size = 14 }: { agentId?: string | null; cli: string | null | undefined; size?: number }) {
    const buddies = useStore((s) => s.faceStyle.on);
    const agent = useStore((s) => (agentId ? s.agents[agentId] : undefined));
    const face = useMemo(() => (agentId ? (agent ?? { id: agentId }) : null), [agent, agentId]);
    if (buddies && face) return <AgentBuddy agent={face} size={size} className="agent-glyph buddy-mark" />;
    const type = agent?.type ?? cli;
    return type && isAgentType(type) ? <AgentIcon type={type} size={size} className={`agent-glyph ${type}`} /> : null;
}
