import { invokeCommand as invoke } from "./invoke";
import type { AgentType } from "../state/types";

export interface ActivityTotals {
    sessions: number;
    resumed: number;
    turns: number;
    agentMs: number;
    commits: number;
    agentCommits: number;
    input: number;
    output: number;
    cacheRead: number;
    cacheWrite: number;
    firstAtMs: number | null;
    /** What the tokens would cost at API prices. Subscription plans bill separately. */
    costUsd: number;
    cacheSavingsUsd: number;
    unpricedTokens: number;
}

export interface ActivityDay {
    /** Local calendar day, counted from the Unix epoch. */
    day: number;
    sessions: number;
    agentMs: number;
    commits: number;
    tokens: number;
    costUsd: number;
}

export interface ActivityShare {
    name: string;
    sessions: number;
    agentMs: number;
    commits: number;
    tokens: number;
    costUsd: number;
}

export interface ActivityModel {
    model: string;
    agent: AgentType;
    input: number;
    output: number;
    cacheRead: number;
    cacheWrite: number;
    costUsd: number;
    fastCostUsd: number;
    priced: boolean;
}

export interface ActivityPricing {
    status: "fresh" | "cached" | "unavailable";
    fetchedAtMs: number | null;
}

/** USD per million tokens. Cache rates fall back to the input rate. */
export interface PriceOverride {
    input: number;
    output: number;
    cacheRead?: number;
    cacheWrite?: number;
}

export interface ActivitySummary {
    totals: ActivityTotals;
    days: ActivityDay[];
    agents: ActivityShare[];
    projects: ActivityShare[];
    models: ActivityModel[];
    pricing: ActivityPricing;
}

export interface ActivityTurnEnd {
    agentId: string;
    agent: AgentType;
    cwd: string;
    sessionId?: string;
    configPath?: string;
}

export const activityApi = {
    turnStarted: (agentId: string, cwd: string): Promise<void> => invoke<void>("activity_turn_started", { agentId, cwd }),
    turnEnded: (turn: ActivityTurnEnd): Promise<void> =>
        invoke<void>("activity_turn_ended", {
            agentId: turn.agentId,
            agent: turn.agent,
            cwd: turn.cwd,
            sessionId: turn.sessionId ?? null,
            configPath: turn.configPath ?? null,
        }),
    summary: (project?: string, prices?: Record<string, PriceOverride>): Promise<ActivitySummary> =>
        invoke<ActivitySummary>("activity_summary", {
            utcOffsetMinutes: -new Date().getTimezoneOffset(),
            project: project ?? null,
            prices: prices ?? null,
        }),
};
