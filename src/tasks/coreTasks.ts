import { useEffect, useState, useSyncExternalStore } from "react";
import { coreCall } from "../api/core";
import { onCoreChanged } from "../api/coreChanged";
import { swallow } from "../state/toast";

/* The shapes below are the core's JSON for tasks (sikemux-core tasks/ and
   server/tasks.rs); the wire generator does not read tagged enums. */

/** Something that would stop a task from starting, worded by the core. */
export type Check = { readonly title: string; readonly detail: string } & (
    | { readonly kind: "buildTool"; readonly ok: boolean; readonly name: string; readonly program: string }
    | { readonly kind: "java"; readonly ok: boolean; readonly required: number; readonly using: number | null; readonly current: number | null }
    | { readonly kind: "deps"; readonly ok: boolean; readonly install: string }
    | {
          readonly kind: "port";
          readonly ok: boolean;
          readonly port: number;
          readonly heldBy: string | null;
          readonly freePort: number | null;
          readonly envVar: string | null;
      }
);

export interface TestFailure {
    readonly className: string;
    readonly name: string;
    readonly message: string;
    /** Absolute path of the test's source file, when it was found. */
    readonly file: string | null;
    readonly line: number | null;
}

export interface TestResults {
    readonly passed: number;
    readonly failed: number;
    readonly skipped: number;
    readonly failures: readonly TestFailure[];
}

export type TaskSourceKind = "project" | "personal" | "detected" | "rerun" | "command";

/** A task the project offers, as the core lists it. */
export interface ProjectTask {
    readonly id: string;
    readonly label: string;
    readonly command: string;
    /** Relative to the project, `.` for its root. */
    readonly cwd: string;
    readonly source: TaskSourceKind;
    readonly java?: { readonly required: number; readonly runsOn: number | null; readonly current: number | null };
}

export interface ProjectTasks {
    readonly project: string;
    readonly configStatus: "absent" | "invalid" | "valid";
    readonly trusted: boolean;
    readonly tasks: readonly ProjectTask[];
}

/** What the core asks the person before it runs commands someone else wrote. */
export interface TrustAsk {
    readonly title: string;
    readonly body: string;
    readonly commands: { label: string; command: string }[];
    readonly confirmLabel: string;
}

/** A trust question waiting on the person. */
export interface PendingTrust extends TrustAsk {
    readonly id: string;
    readonly project: string;
    readonly executionId?: string;
}

export type RunStatus = "awaiting-trust" | "starting" | "running" | "completed" | "failed" | "stopping" | "stopped";

/** One run of a task, whoever started it. */
export interface TaskRun {
    readonly executionId: string;
    readonly taskId: string;
    readonly project: string;
    readonly status: RunStatus;
    readonly label?: string;
    readonly ptyId?: number;
    readonly exitCode?: number;
    readonly signal?: string;
    readonly error?: string;
    readonly previewUrl?: string;
    readonly cwd?: string;
    readonly startedAt?: number;
    /** The agent that started it; none when the person did. */
    readonly agentId: string | null;
    /** Shared by every run of the task, so a rerun shows in the same terminal. */
    readonly terminalKey: string;
    readonly tests?: TestResults;
}

export const ACTIVE_STATUSES: readonly RunStatus[] = ["awaiting-trust", "starting", "running", "stopping"];

export function isActive(run: TaskRun): boolean {
    return ACTIVE_STATUSES.includes(run.status);
}

export const tasksApi = {
    list: (project: string) => coreCall<ProjectTasks>("tasks", { project }),
    /** Answers the checks that stopped the task, or the run it started. */
    start: (project: string, taskId: string, how: { force?: boolean; fix?: boolean } = {}) =>
        coreCall<{ executionId: string } | { checks: Check[] }>("taskStart", { project, taskId, ...how }),
    stop: (executionId: string) => coreCall<void>("taskStop", { executionId }),
    /** Stops every run of a project or an agent that closed. */
    stopRuns: (selector: { project: string } | { agentId: string }) => coreCall<void>("taskStop", selector),
    rerunFailed: (executionId: string) => coreCall<{ taskId: string; executionId: string }>("taskRerunFailed", { executionId }),
    trustAsks: () => coreCall<PendingTrust[]>("trustAsks"),
    answerTrust: (id: string, trust: boolean) => coreCall<void>("answerTrust", { id, trust }),
    /** `replaces` names the task this saves an edit of. */
    add: (project: string, task: { label: string; command: string; cwd: string; team: boolean; replaces?: string }) =>
        coreCall<{ taskId: string }>("addTask", { project, ...task }),
    remove: (project: string, taskId: string) => coreCall<void>("removeTask", { project, taskId }),
    /** Names the project's check command in its sikemux.json; empty takes it out. */
    setCheck: (project: string, command: string) => coreCall<void>("setCheck", { project, command }),
};

let runs: readonly TaskRun[] = [];
let watching = false;
const listeners = new Set<() => void>();

function setRuns(next: readonly TaskRun[]): void {
    runs = next;
    for (const listener of [...listeners]) listener();
}

function readRuns(): void {
    void coreCall<TaskRun[]>("taskRuns").then(setRuns).catch(swallow("read task runs"));
}

export function setTaskRunsForTests(next: readonly TaskRun[]): void {
    watching = true;
    setRuns(next);
}

function subscribeRuns(listener: () => void): () => void {
    listeners.add(listener);
    if (!watching) {
        watching = true;
        onCoreChanged("runs", readRuns);
        readRuns();
    }
    return () => listeners.delete(listener);
}

/** Every run the core knows, as the core last said. */
export const taskRuns = { subscribe: subscribeRuns, getSnapshot: (): readonly TaskRun[] => runs };

export function useTaskRuns(): readonly TaskRun[] {
    return useSyncExternalStore(taskRuns.subscribe, taskRuns.getSnapshot, taskRuns.getSnapshot);
}

/** The newest run with a process behind `terminalKey`, which its terminal shows. */
export function runShownIn(all: readonly TaskRun[], terminalKey: string): TaskRun | null {
    for (let index = all.length - 1; index >= 0; index -= 1) {
        const run = all[index];
        if (run.terminalKey === terminalKey && run.ptyId !== undefined) return run;
    }
    return null;
}

/** What `project` can run, read again whenever the core says it changed. */
export function useProjectTasks(project: string | null, refreshKey: unknown = null): ProjectTasks | null {
    const [found, setFound] = useState<ProjectTasks | null>(null);
    const [version, setVersion] = useState(0);
    useEffect(() => {
        if (!project) return;
        return onCoreChanged("tasks", (change) => {
            if (change.project === null || change.project === project) setVersion((current) => current + 1);
        });
    }, [project]);
    useEffect(() => {
        if (!project) return;
        let cancelled = false;
        void tasksApi
            .list(project)
            .then((next) => {
                if (!cancelled) setFound(next);
            })
            .catch(swallow("list project tasks"));
        return () => {
            cancelled = true;
        };
    }, [project, version, refreshKey]);
    return found?.project === project ? found : null;
}
