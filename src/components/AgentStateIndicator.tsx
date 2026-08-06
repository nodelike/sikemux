import { CircleAlert, CircleCheck, CircleDot, CircleHelp, LoaderCircle, type LucideIcon } from "lucide-react";
import { AGENT_STATE_META } from "../state/agentStatus";
import type { AgentPresentationState } from "../state/types";

const STATE_ICONS: Record<AgentPresentationState, LucideIcon> = {
    working: LoaderCircle,
    blocked: CircleAlert,
    done: CircleCheck,
    idle: CircleDot,
    unknown: CircleHelp,
};

export function AgentStateIndicator({ state, unread = false }: { state: AgentPresentationState; unread?: boolean }) {
    const Icon = STATE_ICONS[state];
    const label = AGENT_STATE_META[state].label;
    return (
        <span className={`agent-activity state-${state}${unread ? " unread" : ""}`} title={label} aria-label={label} role="img">
            <Icon className="agent-state-icon" size={13} strokeWidth={2.15} aria-hidden="true" />
        </span>
    );
}
