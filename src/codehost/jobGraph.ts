import type { Job, LogLine, Step } from "./api";

/** Jobs from one called workflow, which GitHub names `Caller / Job` and draws in one box. */
export interface JobGroup {
    key: string;
    title: string | null;
    jobs: { job: Job; label: string }[];
}

export type Stage = JobGroup[];

/** A matrix job's values are in parentheses and may hold " / " themselves, so only a split outside them counts. */
export function splitName(name: string): { group: string | null; label: string } {
    let depth = 0;
    for (let at = 0; at < name.length; at += 1) {
        const char = name[at];
        if (char === "(") depth += 1;
        else if (char === ")") depth = Math.max(0, depth - 1);
        else if (depth === 0 && at > 0 && name.startsWith(" / ", at)) return { group: name.slice(0, at), label: name.slice(at + 3) };
    }
    return { group: null, label: name };
}

function time(value: string | null): number | null {
    if (!value) return null;
    const at = Date.parse(value);
    return Number.isNaN(at) ? null : at;
}

interface Unit {
    group: JobGroup;
    start: number | null;
    end: number | null;
    order: number;
}

/**
 * GitHub does not say which job waited for which, so the stages are read off
 * the clock: a group that only started once everything before it had finished
 * was waiting on it. Jobs that have not started yet come last.
 */
export function stagesOf(jobs: readonly Job[]): Stage[] {
    const units = new Map<string, Unit>();
    jobs.forEach((job, order) => {
        const { group, label } = splitName(job.name);
        const key = group ?? `job:${job.id}`;
        const unit = units.get(key) ?? { group: { key, title: group, jobs: [] }, start: null, end: null, order };
        unit.group.jobs.push({ job, label: group ? label : job.name });
        const start = time(job.startedAt);
        const end = time(job.completedAt);
        if (start !== null) unit.start = unit.start === null ? start : Math.min(unit.start, start);
        if (end !== null && job.status === "completed") unit.end = unit.end === null ? end : Math.max(unit.end, end);
        else if (job.status !== "completed") unit.end = Number.POSITIVE_INFINITY;
        units.set(key, unit);
    });

    const started = [...units.values()].filter((unit) => unit.start !== null);
    const waiting = [...units.values()].filter((unit) => unit.start === null);
    started.sort((a, b) => (a.start ?? 0) - (b.start ?? 0) || a.order - b.order);
    waiting.sort((a, b) => a.order - b.order);

    const stages: Stage[] = [];
    let current: Stage = [];
    let currentEnd = Number.NEGATIVE_INFINITY;
    for (const unit of started) {
        if (current.length > 0 && (unit.start ?? 0) >= currentEnd) {
            stages.push(current);
            current = [];
            currentEnd = Number.NEGATIVE_INFINITY;
        }
        current.push(unit.group);
        currentEnd = Math.max(currentEnd, unit.end ?? Number.POSITIVE_INFINITY);
    }
    if (current.length > 0) stages.push(current);
    if (waiting.length > 0) stages.push(waiting.map((unit) => unit.group));
    return stages;
}

/**
 * Where each step's part of the log begins. A job's log is one stream, and the
 * only thing tying a line to a step is when it was written.
 */
export function stepStarts(lines: readonly LogLine[], steps: readonly Step[]): Map<number, number> {
    const starts = new Map<number, number>();
    const stamps = lines.map((line) => time(line.timestamp));
    let from = 0;
    for (const step of steps) {
        const begins = time(step.startedAt);
        if (begins === null) continue;
        // A step's time is kept to the second and a line's to a fraction of one.
        const floor = Math.floor(begins / 1000) * 1000;
        let index = from;
        while (index < stamps.length && (stamps[index] ?? Number.NEGATIVE_INFINITY) < floor) index += 1;
        if (index >= stamps.length) break;
        starts.set(step.number, index);
        from = index;
    }
    return starts;
}
