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
    const worked = event.state === "working" || event.state === "blocked";
    const lastWorkedAt = worked ? now : previous?.lastWorkedAt;
    return {
        state,
        backendState: event.state,
        unread,
        updatedAt: now,
        ...(lastWorkedAt != null ? { lastWorkedAt } : {}),
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

const PRIORITY: Record<AgentPresentationState, number> = { blocked: 6, working: 5, done: 4, stopped: 3, unknown: 2, idle: 1 };

export function rollupAgentStates(values: Array<AgentRuntimeState | undefined>): AgentPresentationState | undefined {
    let best: AgentPresentationState | undefined;
    for (const value of values) if (value && (!best || PRIORITY[value.state] > PRIORITY[best])) best = value.state;
    return best;
}

/**
 * Open agents in the order they want attention: waiting on you, running,
 * finished unseen, then the rest by how recently they last ran.
 */
export function sortByAttention<T extends { id: string }>(
    agents: T[],
    activityById: Record<string, AgentRuntimeState | undefined>,
    backgroundById: Record<string, number | undefined>,
): T[] {
    const tier = (a: T) => {
        const state = activityById[a.id]?.state;
        if (state === "blocked") return 3;
        if (state === "working" || (backgroundById[a.id] ?? 0) > 0) return 2;
        if (state === "done") return 1;
        return 0;
    };
    const lastWorked = (a: T) => activityById[a.id]?.lastWorkedAt ?? 0;
    return agents
        .map((agent) => ({ agent, tier: tier(agent), lastWorked: lastWorked(agent) }))
        .sort((x, y) => y.tier - x.tier || (x.tier >= 2 ? 0 : y.lastWorked - x.lastWorked))
        .map(({ agent }) => agent);
}

export const AGENT_STATE_META: Record<AgentPresentationState, { label: string }> = {
    working: { label: "Working" },
    blocked: { label: "Needs input" },
    done: { label: "Done — unseen" },
    idle: { label: "Ready" },
    stopped: { label: "Stopped" },
    unknown: { label: "Unknown" },
};
