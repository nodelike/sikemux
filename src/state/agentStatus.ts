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

export type AgentGroup = "blocked" | "working" | "done" | "idle";

/** The groups in the order they want attention, and the one word each surface uses for them. */
export const AGENT_GROUPS: readonly AgentGroup[] = ["blocked", "working", "done", "idle"];

export const AGENT_LABELS: Record<AgentGroup, string> = {
    blocked: "Needs input",
    working: "Working",
    done: "Done",
    idle: "Idle",
};

const RANK: Record<AgentGroup, number> = { blocked: 3, working: 2, done: 1, idle: 0 };

export interface AgentBackground {
    subagents?: number;
    tasks?: number;
}

export interface AgentDisplay {
    state: AgentPresentationState;
    group: AgentGroup;
    subagents: number;
    tasks: number;
    label: string;
    rank: number;
}

/**
 * How an agent is shown everywhere. Its state counts subagents still at work
 * as Working; shells left running only show as counts.
 */
export function agentDisplay(state: AgentPresentationState | undefined, background: AgentBackground = {}): AgentDisplay {
    const shown = state ?? "idle";
    const group: AgentGroup = shown === "blocked" || shown === "working" || shown === "done" ? shown : "idle";
    return { state: shown, group, subagents: background.subagents ?? 0, tasks: background.tasks ?? 0, label: AGENT_LABELS[group], rank: RANK[group] };
}

/** A group of agents shown as the one that wants attention most, counting everything they still have going. */
export function rollupAgentDisplays(displays: readonly AgentDisplay[]): AgentDisplay | undefined {
    let best: AgentDisplay | undefined;
    let subagents = 0;
    let tasks = 0;
    for (const display of displays) {
        if (!best || display.rank > best.rank) best = display;
        subagents += display.subagents;
        tasks += display.tasks;
    }
    return best && { ...best, subagents, tasks };
}

/** Mid-turn or waiting on an answer: stopping it now loses work, so closing it asks first. */
export const isAgentBusy = (activity: AgentRuntimeState | undefined): boolean =>
    activity?.backendState === "working" || activity?.backendState === "blocked";

export const lastWorkedAt = (activity: AgentRuntimeState | undefined): number => activity?.lastWorkedAt ?? 0;
