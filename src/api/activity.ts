import { coreCall } from "./core";
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
    /** How full the calendar draws the day for each measure, 0 to 4. */
    levels: ActivityLevels;
}

export interface ActivityLevels {
    sessions: number;
    agentMs: number;
    commits: number;
    tokens: number;
    costUsd: number;
}

/** Days in a row with any activity; a streak that reached yesterday still counts as current. */
export interface ActivityStreaks {
    activeDays: number;
    longest: number;
    current: number;
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
    fetchedAtMs: number | null;
    /** The person's own prices, by model. */
    overrides: Record<string, PriceOverride>;
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
    streaks: ActivityStreaks;
    agents: ActivityShare[];
    projects: ActivityShare[];
    models: ActivityModel[];
    pricing: ActivityPricing;
}

export const activityApi = {
    summary: (project?: string): Promise<ActivitySummary> =>
        coreCall<ActivitySummary>("activitySummary", { utcOffsetMinutes: -new Date().getTimezoneOffset(), project: project ?? null }),
    /** Prices `model` at what the person entered, or at the published price again with none. */
    setPrice: (model: string, price: PriceOverride | null): Promise<void> => coreCall<void>("setActivityPrice", { model, price }),
};
