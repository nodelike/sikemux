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
}

export interface ActivityDay {
    /** Local calendar day, counted from the Unix epoch. */
    day: number;
    sessions: number;
    agentMs: number;
    commits: number;
    tokens: number;
}

export interface ActivityShare {
    name: string;
    sessions: number;
    agentMs: number;
    commits: number;
    tokens: number;
}

export interface ActivitySummary {
    totals: ActivityTotals;
    days: ActivityDay[];
    agents: ActivityShare[];
    projects: ActivityShare[];
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
    summary: (): Promise<ActivitySummary> => invoke<ActivitySummary>("activity_summary", { utcOffsetMinutes: -new Date().getTimezoneOffset() }),
};
