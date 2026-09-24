import { AGENT_STATE_META } from "../state/agentStatus";
import type { AgentPresentationState } from "../state/types";
import { IconAgent, IconCommand } from "./Icons";

const BACKGROUND_LABEL = "Shells or monitors still running";

/**
 * A spinner while an agent works, a dot once it has something waiting for you,
 * and nothing while it sits idle — a rail of idle agents would be a column of
 * dots carrying no information, since the row already says the agent exists.
 * Left-over shells get the terminal glyph instead of a dot, so they do not read
 * as the same amber dot that means the agent is waiting on you.
 */
export function showsAgentState(state: AgentPresentationState, background = false): boolean {
    return state === "working" || state === "blocked" || state === "done" || background;
}

export function AgentStateIndicator({
    state,
    unread = false,
    background = false,
}: {
    state: AgentPresentationState;
    unread?: boolean;
    background?: boolean;
}) {
    if (state === "working") {
        const label = AGENT_STATE_META.working.label;
        return (
            <span className={`agent-activity state-working${unread ? " unread" : ""}`} title={label} aria-label={label} role="img">
                <span className="agent-state-loader" aria-hidden="true" />
            </span>
        );
    }
    if (!showsAgentState(state, background)) return null;
    const tone = state === "blocked" ? "blocked" : state === "done" ? "done" : "background";
    const label = tone === "background" ? BACKGROUND_LABEL : AGENT_STATE_META[state].label;
    return (
        <span className={`agent-activity state-${tone}${unread ? " unread" : ""}`} title={label} aria-label={label} role="img">
            {tone === "background" ? <IconCommand size={11} className="agent-state-icon" /> : <span className="agent-state-dot" aria-hidden="true" />}
        </span>
    );
}

export function SubagentCount({ count }: { count: number }) {
    const label = `${count} ${count === 1 ? "subagent" : "subagents"} running`;
    return (
        <span className="subagent-count" title={label} aria-label={label} role="img">
            <IconAgent size={16} />
            <span className="subagent-count-dot" aria-hidden="true">
                {count}
            </span>
        </span>
    );
}
