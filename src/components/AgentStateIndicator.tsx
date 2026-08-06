import { CircleAlert, CircleCheck, CircleDot, CircleHelp, type LucideIcon } from "lucide-react";
import { AGENT_STATE_META } from "../state/agentStatus";
import type { AgentPresentationState } from "../state/types";

const STATE_ICONS: Record<Exclude<AgentPresentationState, "working">, LucideIcon> = {
    blocked: CircleAlert,
    done: CircleCheck,
    idle: CircleDot,
    unknown: CircleHelp,
};

export function AgentStateIndicator({ state, unread = false }: { state: AgentPresentationState; unread?: boolean }) {
    const label = AGENT_STATE_META[state].label;
    const Icon = state === "working" ? null : STATE_ICONS[state];
    return (
        <span className={`agent-activity state-${state}${unread ? " unread" : ""}`} title={label} aria-label={label} role="img">
            {Icon ? (
                <Icon className="agent-state-icon" size={13} strokeWidth={2.15} aria-hidden="true" />
            ) : (
                <span className="agent-state-loader" aria-hidden="true" />
            )}
        </span>
    );
}
