import type { Job, Run, RunStatus, Step } from "./api";
import type { StatusFilter } from "./state";

export type Outcome = "running" | "queued" | "success" | "failure" | "cancelled" | "skipped" | "blocked" | "unknown";

const BY_CONCLUSION: Record<string, Outcome> = {
    success: "success",
    failure: "failure",
    timed_out: "failure",
    startup_failure: "failure",
    cancelled: "cancelled",
    stale: "cancelled",
    skipped: "skipped",
    neutral: "skipped",
    action_required: "blocked",
};

const BY_STATUS: Record<string, Outcome> = {
    in_progress: "running",
    queued: "queued",
    requested: "queued",
    pending: "queued",
    waiting: "blocked",
    action_required: "blocked",
};

export function outcomeOf(thing: Pick<Run, "status" | "conclusion">): Outcome {
    if (thing.status === "completed") return BY_CONCLUSION[thing.conclusion ?? ""] ?? "unknown";
    return BY_STATUS[thing.status] ?? "unknown";
}

/** Still going, including a run held for approval, which can still be cancelled but not re-run. */
export function isUnfinished(thing: Pick<Run, "status" | "conclusion">): boolean {
    if (thing.status === "completed") return false;
    const outcome = outcomeOf(thing);
    return outcome === "running" || outcome === "queued" || outcome === "blocked";
}

/** Skipped and neutral checks count as passing, the way GitHub counts them. */
export function checksSummary(runs: readonly Pick<Run, "status" | "conclusion">[]): string {
    const tally: Record<Outcome, number> = { running: 0, queued: 0, success: 0, failure: 0, cancelled: 0, skipped: 0, blocked: 0, unknown: 0 };
    for (const run of runs) tally[outcomeOf(run)] += 1;
    const passed = tally.success + tally.skipped;
    if (passed === runs.length) return "all passed";
    const parts: [number, string][] = [
        [tally.failure, "failing"],
        [tally.running + tally.queued, "running"],
        [tally.blocked, "waiting"],
        [tally.cancelled, "cancelled"],
        [tally.unknown, "unknown"],
        [passed, "passed"],
    ];
    return parts
        .filter(([count]) => count > 0)
        .map(([count, word]) => `${count} ${word}`)
        .join(", ");
}

/** One outcome for a commit's runs: any failure fails it, then anything still going keeps it running. */
export function overallOutcome(runs: readonly Pick<Run, "status" | "conclusion">[]): Outcome {
    const outcomes = runs.map(outcomeOf);
    if (outcomes.includes("failure")) return "failure";
    if (outcomes.some((outcome) => outcome === "running" || outcome === "queued")) return "running";
    if (outcomes.includes("blocked")) return "blocked";
    return "success";
}

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

export function formatDuration(ms: number | null): string {
    if (ms === null) return "—";
    const seconds = Math.round(ms / 1000);
    if (seconds < 60) return `${seconds}s`;
    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) return `${minutes}m ${seconds % 60}s`;
    return `${Math.floor(minutes / 60)}h ${minutes % 60}m`;
}

const AGO: [limit: number, unit: string, per: number][] = [
    [60, "s", 1],
    [3600, "m", 60],
    [86_400, "h", 3600],
    [2_592_000, "d", 86_400],
];

export function formatAgo(timestamp: string | null, now: number): string {
    if (!timestamp) return "—";
    const at = Date.parse(timestamp);
    if (Number.isNaN(at)) return "—";
    const seconds = Math.max(0, Math.round((now - at) / 1000));
    for (const [limit, unit, per] of AGO) {
        if (seconds < limit) return `${Math.floor(seconds / per)}${unit} ago`;
    }
    return `${Math.floor(seconds / 2_592_000)}mo ago`;
}

export function failedStep(job: Job): Step | undefined {
    return job.steps.find((step) => outcomeOf(step) === "failure");
}

export function jobsSummary(jobs: Job[]): { done: number; total: number; failed: number } {
    let done = 0;
    let failed = 0;
    for (const job of jobs) {
        if (job.status === "completed") done += 1;
        if (outcomeOf(job) === "failure") failed += 1;
    }
    return { done, total: jobs.length, failed };
}

export const SUMMARY_LIMIT = 10;

/**
 * The jobs whose summaries are worth a request. Each one is its own call, so a
 * run still going waits until it ends, and a wide matrix only loads the first few.
 */
export function summaryJobs(jobs: readonly Job[], runFinished: boolean, everything: boolean): Job[] {
    if (!runFinished) return [];
    const done = jobs.filter((job) => job.status === "completed" && job.checkRunId !== null && job.conclusion !== "skipped");
    return everything ? done : done.slice(0, SUMMARY_LIMIT);
}

/**
 * Whether what a watch last pushed is newer than what was just read. A re-run
 * is a later attempt, and a refresh reads the same attempt again, so the read
 * wins unless the watch has seen something since.
 */
export function watchIsNewer(watched: Pick<Run, "attempt" | "updatedAt"> | null, read: Pick<Run, "attempt" | "updatedAt"> | null): boolean {
    if (!watched) return false;
    if (!read) return true;
    if (watched.attempt !== read.attempt) return watched.attempt > read.attempt;
    return Date.parse(watched.updatedAt) > Date.parse(read.updatedAt);
}

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
