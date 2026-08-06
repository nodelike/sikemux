import type { AgentBackendState, AgentPresentationState, AgentRuntimeState } from "./types";

export interface AgentStateEvent {
    agentId: string;
    state: AgentBackendState;
    sequence: number;
    source: AgentRuntimeState["source"];
    confidence: AgentRuntimeState["confidence"];
    reason: string;
    matchedRule?: string;
}

export function reduceAgentState(
    previous: AgentRuntimeState | undefined,
    event: AgentStateEvent,
    visible: boolean,
    now = Date.now(),
): AgentRuntimeState | undefined {
    if (previous && event.sequence <= previous.sequence) return undefined;
    let state: AgentPresentationState = event.state;
    let unread = false;
    if (event.state === "idle" && !visible && (previous?.backendState === "working" || previous?.backendState === "blocked")) {
        state = "done";
        unread = true;
    } else if (event.state === "blocked") {
        unread = !visible;
    }
    return {
        state,
        backendState: event.state,
        unread,
        updatedAt: now,
        sequence: event.sequence,
        source: event.source,
        confidence: event.confidence,
        reason: event.reason,
        ...(event.matchedRule ? { matchedRule: event.matchedRule } : {}),
    };
}

export function acknowledgeAgentState(value: AgentRuntimeState): AgentRuntimeState {
    return { ...value, state: value.state === "done" ? "idle" : value.state, unread: false };
}

const PRIORITY: Record<AgentPresentationState, number> = { blocked: 5, working: 4, done: 3, unknown: 2, idle: 1 };

export function rollupAgentStates(values: Array<AgentRuntimeState | undefined>): AgentPresentationState | undefined {
    let best: AgentPresentationState | undefined;
    for (const value of values) if (value && (!best || PRIORITY[value.state] > PRIORITY[best])) best = value.state;
    return best;
}

export const AGENT_STATE_META: Record<AgentPresentationState, { label: string }> = {
    working: { label: "Working" },
    blocked: { label: "Needs input" },
    done: { label: "Done — unseen" },
    idle: { label: "Idle" },
    unknown: { label: "Unknown" },
};
