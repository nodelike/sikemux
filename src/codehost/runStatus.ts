import type { CheckTally, Outcome, Run, RunStatus } from "./api";
import type { StatusFilter } from "./state";

export type { Outcome };

/** What the host's status and conclusion come to, as the plugin worked it out. */
export function outcomeOf(thing: Pick<Run, "outcome">): Outcome {
    return thing.outcome;
}

/** Skipped and neutral checks count as passing, the way GitHub counts them. */
/** A commit's runs, counted by the core, in words. */
export function tallySummary(tally: CheckTally): string {
    if (tally.passed === tally.total) return "all passed";
    const parts: [number, string][] = [
        [tally.failing, "failing"],
        [tally.running + tally.queued, "running"],
        [tally.waiting, "waiting"],
        [tally.cancelled, "cancelled"],
        [tally.unknown, "unknown"],
        [tally.passed, "passed"],
    ];
    return parts
        .filter(([count]) => count > 0)
        .map(([count, word]) => `${count} ${word}`)
        .join(", ");
}

/** One outcome for a commit's runs: any failure fails it, then anything still going keeps it running. */
export const OUTCOME_LABEL: Record<Outcome, string> = {
    running: "Running",
    queued: "Queued",
    success: "Passed",
    failure: "Failed",
    cancelled: "Cancelled",
    skipped: "Skipped",
    blocked: "Waiting for approval",
    unknown: "Unknown",
};

export function statusParam(filter: StatusFilter): RunStatus | undefined {
    return filter === "all" ? undefined : filter;
}

export function elapsedMs(started: string | null, finished: string | null, now: number): number | null {
    if (!started) return null;
    const from = Date.parse(started);
    if (Number.isNaN(from)) return null;
    const to = finished ? Date.parse(finished) : now;
    if (Number.isNaN(to)) return null;
    return Math.max(0, to - from);
}

/** Each job's summary is its own call, so only this many load until the person asks for the rest. */
export const SUMMARY_LIMIT = 10;

const EVENT_LABEL: Record<string, string> = {
    push: "Push",
    pull_request: "Pull request",
    pull_request_target: "Pull request",
    workflow_dispatch: "Manual",
    schedule: "Scheduled",
    release: "Release",
    workflow_run: "Workflow run",
    merge_group: "Merge queue",
};

export function eventLabel(event: string): string {
    const words = event.replace(/_/gu, " ");
    return EVENT_LABEL[event] ?? words.charAt(0).toUpperCase() + words.slice(1);
}
