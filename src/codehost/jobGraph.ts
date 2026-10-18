import type { Job, LogLine, Step } from "./api";
import { outcomeOf } from "./runStatus";
import { calledPath, type WorkflowJob } from "./workflowJobs";

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

interface Named {
    job: Job;
    name: string;
    order: number;
}

/**
 * The list of jobs does not say which job waited for which, so without the
 * workflow file the stages are read off the clock: a group that only started
 * once everything before it had finished was waiting on it. Jobs that have not
 * started yet come last.
 */
export function stagesOf(jobs: readonly Job[]): Stage[] {
    return stagesOfNamed(jobs.map((job, order) => ({ job, name: job.name, order })));
}

function stagesOfNamed(named: readonly Named[]): Stage[] {
    const units = new Map<string, Unit>();
    named.forEach(({ job, name }, order) => {
        const { group, label } = splitName(name);
        const key = group ?? `job:${job.id}`;
        const unit = units.get(key) ?? { group: { key, title: group, jobs: [] }, start: null, end: null, order };
        unit.group.jobs.push({ job, label: group ? label : name });
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

export interface GraphJob {
    /** Null for a job the file declares that the run has not made yet. */
    job: Job | null;
    label: string;
    /** The job that called its workflow was skipped, so it never will run. */
    skipped: boolean;
}

/** A lone job, a matrix job's runs, or jobs that wait for the same jobs and are waited on by the same jobs. */
export interface GraphBox {
    id: string;
    /** A matrix job's name; null otherwise. */
    title: string | null;
    jobs: GraphJob[];
}

export interface Graph {
    /** Each column comes after every column it waits for. */
    columns: GraphBox[][];
    /** From a box to each box that waits for it. */
    edges: [string, string][];
}

/** Each workflow file's jobs by its path, or null when it could not be read. */
export type WorkflowFiles = Record<string, WorkflowJob[] | null>;

const EXPRESSION = /\$\{\{[\s\S]*?\}\}/g;

function displayName(job: WorkflowJob): string {
    return job.name ?? job.key;
}

/** A name with its `${{ … }}` parts taken out, for a job that has not run yet. */
function plainName(name: string): string {
    const plain = name
        .replace(/\s*\(\s*\$\{\{[\s\S]*?\}\}[^)]*\)/g, "")
        .replace(EXPRESSION, "")
        .replace(/\s{2,}/g, " ")
        .trim();
    return plain || name;
}

/** GitHub fills in a name's expressions, or adds a matrix job's values after it in parentheses. */
function matcher(display: string): (name: string) => boolean {
    const parts = display.split(EXPRESSION);
    if (parts.length === 1) return (name) => name === display || (name.startsWith(`${display} (`) && name.endsWith(")"));
    const pattern = new RegExp(`^${parts.map((part) => part.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("[\\s\\S]*?")}$`);
    return (name) => pattern.test(name);
}

function matrixLabel(name: string, title: string): string {
    if (name.startsWith(`${title} (`) && name.endsWith(")")) return name.slice(title.length + 2, -1);
    return name;
}

interface Part {
    id: string;
    title: string | null;
    jobs: GraphJob[];
    needs: string[];
    /** Where the run lists it, which is the order GitHub stacks jobs that wait for nothing. */
    order: number;
}

interface Level {
    parts: Part[];
    /** The parts nothing else in the same file waits for, which stand for the file to a job that needs its caller. */
    leaves: string[];
}

function earliest(records: readonly Named[]): number {
    return Math.min(Number.POSITIVE_INFINITY, ...records.map((record) => record.order));
}

/**
 * A called workflow's jobs join the run's own columns, named `Caller / Job`: the
 * ones that wait for nothing in their file wait for what the caller needs, and
 * a job that needs the caller waits for the ones nothing in that file waits for.
 */
function fromFile(
    named: readonly Named[],
    declared: readonly WorkflowJob[],
    files: WorkflowFiles,
    prefix: string,
    labelPrefix: string,
    skipped: boolean,
    order: number,
): Level | null {
    const matches = declared.map((each) => matcher(displayName(each)));
    const own = declared.map((): Named[] => []);
    const called = declared.map((): Named[] => []);
    for (const entry of named) {
        const { group, label } = splitName(entry.name);
        const caller = group === null ? -1 : declared.findIndex((each, index) => each.uses !== null && matches[index]?.(group));
        if (caller >= 0) {
            called[caller]?.push({ ...entry, name: label });
            continue;
        }
        const exact = declared.findIndex((each) => displayName(each) === entry.name);
        const at = exact >= 0 ? exact : matches.findIndex((test) => test(entry.name));
        if (at < 0) return null;
        own[at]?.push(entry);
    }

    const parts: Part[] = [];
    const entries = new Map<string, Part[]>();
    const exits = new Map<string, string[]>();
    declared.forEach((each, index) => {
        const id = prefix + each.key;
        const records = own[index] ?? [];
        const calls = called[index] ?? [];
        const title = plainName(displayName(each));
        const rank = Math.min(earliest(records), earliest(calls), order);
        if (each.uses !== null) {
            const callerSkipped = skipped || records.some((record) => outcomeOf(record.job) === "skipped");
            const path = calledPath(each.uses);
            const file = path ? files[path] : null;
            const inner =
                (file && fromFile(calls, file, files, `${id}/`, `${labelPrefix}${title} / `, callerSkipped, rank)) ||
                (calls.length > 0 ? unordered(calls, `${id}/`, `${labelPrefix}${title} / `) : null);
            if (inner) {
                parts.push(...inner.parts);
                entries.set(
                    each.key,
                    inner.parts.filter((part) => part.needs.length === 0),
                );
                exits.set(each.key, inner.leaves);
                return;
            }
        }
        const jobs: GraphJob[] =
            records.length > 1
                ? records.map((record) => ({ job: record.job, label: matrixLabel(record.name, title), skipped: false }))
                : [{ job: records[0]?.job ?? null, label: labelPrefix + (records[0]?.name ?? title), skipped }];
        const part: Part = { id, title: records.length > 1 ? labelPrefix + title : null, jobs, needs: [], order: rank };
        parts.push(part);
        entries.set(each.key, [part]);
        exits.set(each.key, [id]);
    });
    for (const each of declared) {
        const needs = each.needs.flatMap((need) => exits.get(need) ?? []);
        for (const part of entries.get(each.key) ?? []) part.needs.push(...needs);
    }
    const needed = new Set(parts.flatMap((part) => part.needs));
    return { parts, leaves: parts.filter((part) => !needed.has(part.id)).map((part) => part.id) };
}

/** A called workflow whose file could not be read: its jobs side by side, all waiting for what the caller needs. */
function unordered(calls: readonly Named[], prefix: string, labelPrefix: string): Level {
    const parts = calls.map((call): Part => ({
        id: `${prefix}${call.job.id}`,
        title: null,
        jobs: [{ job: call.job, label: labelPrefix + call.name, skipped: false }],
        needs: [],
        order: call.order,
    }));
    return { parts, leaves: parts.map((part) => part.id) };
}

/** Jobs the run has made keep the run's order; the ones it has not go after them by name. */
function rowOrder(listed: ReadonlyMap<string, number>): (a: GraphJob, b: GraphJob) => number {
    const rank = (row: GraphJob) => (row.job ? (listed.get(row.job.id) ?? 0) : Number.POSITIVE_INFINITY);
    return (a, b) => rank(a) - rank(b) || (a.label < b.label ? -1 : a.label > b.label ? 1 : 0);
}

function depthsOf(parts: readonly Part[]): Map<string, number> {
    const byId = new Map(parts.map((part) => [part.id, part]));
    const depths = new Map<string, number>();
    const depthOf = (part: Part, seen: Set<string>): number => {
        const known = depths.get(part.id);
        if (known !== undefined) return known;
        if (seen.has(part.id)) return 0;
        seen.add(part.id);
        let depth = 0;
        for (const need of part.needs) {
            const before = byId.get(need);
            if (before) depth = Math.max(depth, depthOf(before, seen) + 1);
        }
        depths.set(part.id, depth);
        return depth;
    };
    for (const part of parts) depthOf(part, new Set());
    return depths;
}

/**
 * GitHub's columns: each job one column after the last of the jobs it needs. Jobs
 * in a column that wait for the same jobs and are waited on by the same jobs share
 * a box, and each column is stacked by where the boxes it waits for sit, so the
 * lines stay short and do not cross.
 */
function layout(parts: readonly Part[], listed: ReadonlyMap<string, number>): Graph {
    const depths = depthsOf(parts);
    const dependents = new Map<string, string[]>();
    for (const part of parts) for (const need of part.needs) dependents.set(need, [...(dependents.get(need) ?? []), part.id]);
    const ids = (list: readonly string[]) => [...new Set(list)].sort().join(",");

    const boxOf = new Map<string, string>();
    const boxes = new Map<string, GraphBox & { depth: number; order: number; needs: Set<string> }>();
    for (const part of parts) {
        const depth = depths.get(part.id) ?? 0;
        const key = part.title === null ? `${depth}|${ids(part.needs)}|${ids(dependents.get(part.id) ?? [])}` : part.id;
        const box = boxes.get(key);
        if (box) {
            box.jobs.push(...part.jobs);
            box.order = Math.min(box.order, part.order);
        } else {
            boxes.set(key, { id: part.id, title: part.title, jobs: [...part.jobs], depth, order: part.order, needs: new Set() });
        }
        boxOf.set(part.id, (boxes.get(key) as GraphBox).id);
    }
    const byId = new Map([...boxes.values()].map((box) => [box.id, box]));

    const edges = new Map<string, [string, string]>();
    for (const part of parts) {
        const to = boxOf.get(part.id) as string;
        for (const need of part.needs) {
            const from = boxOf.get(need);
            if (!from || from === to) continue;
            edges.set(`${from}>${to}`, [from, to]);
            byId.get(to)?.needs.add(from);
        }
    }

    const columns: (GraphBox & { order: number; needs: Set<string> })[][] = [];
    for (const box of byId.values()) (columns[box.depth] ??= []).push(box);
    const at = new Map<string, number>();
    const placed = columns.filter(Boolean).map((column) => {
        const weight = (box: { needs: Set<string> }) =>
            box.needs.size === 0 ? 0 : [...box.needs].reduce((sum, need) => sum + (at.get(need) ?? 0), 0) / box.needs.size;
        const sorted = [...column].sort((a, b) => weight(a) - weight(b) || a.order - b.order);
        let top = 0;
        for (const box of sorted) {
            at.set(box.id, top);
            top += box.jobs.length + (box.title ? 1 : 0) + 1;
        }
        return sorted.map(({ id, title, jobs }): GraphBox => ({ id, title, jobs: title ? jobs : jobs.sort(rowOrder(listed)) }));
    });
    return { columns: placed, edges: [...edges.values()] };
}

/** Without the workflow file, every box in a stage is drawn as waiting for every box in the one before it. */
function byClock(named: readonly Named[]): Graph {
    const columns = stagesOfNamed(named).map((stage) =>
        stage.map((group): GraphBox => ({
            id: group.key,
            title: null,
            jobs: group.jobs.map(({ job }) => ({ job, label: job.name, skipped: false })),
        })),
    );
    const edges = columns
        .slice(1)
        .flatMap((column, index) => (columns[index] ?? []).flatMap((from) => column.map((to): [string, string] => [from.id, to.id])));
    return { columns, edges };
}

/**
 * The run's jobs laid out as GitHub draws them, by what each one `needs` in the
 * workflow file at `path`, with a called workflow's jobs in the run's own columns.
 * Without the file, or with a job it does not declare, the stages are read off the clock.
 */
export function graphOf(jobs: readonly Job[], files: WorkflowFiles | null, path: string | null): Graph {
    const named = jobs.map((job, order) => ({ job, name: job.name, order }));
    const declared = files && path ? files[path] : null;
    const level = declared && files ? fromFile(named, declared, files, "", "", false, Number.POSITIVE_INFINITY) : null;
    return level ? layout(level.parts, new Map(jobs.map((job, order) => [job.id, order]))) : byClock(named);
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
