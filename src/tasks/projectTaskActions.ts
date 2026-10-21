import { externalValue, useExternalValue } from "../lib/externalValue";
import { notify, reportError } from "../state/toast";
import { isActive, tasksApi, type Check, type ProjectTask, type TaskRun } from "./coreTasks";
import { focusWhenShown } from "./taskTerminals";

/** A run held back by a failed check, waiting for the person to fix it or run anyway. */
export interface PendingCheck {
    readonly project: string;
    readonly task: ProjectTask;
    readonly checks: readonly Check[];
}

const pendingCheck = externalValue<PendingCheck | null>(null);

export const usePendingCheck = (): PendingCheck | null => useExternalValue(pendingCheck);

export function dismissPendingCheck(): void {
    pendingCheck.set(null);
}

/**
 * Starts the task in place of whatever the person runs in the project, or of
 * its own run, once the core's checks pass. `force` starts it anyway; `fix`
 * has the core install what is missing first.
 */
export function runProjectTask(project: string, task: ProjectTask, how: { force?: boolean; fix?: boolean } = {}): Promise<void> {
    return tasksApi
        .start(project, task.id, how)
        .then((answer) => {
            if ("checks" in answer) {
                pendingCheck.set({ project, task, checks: answer.checks });
                return;
            }
            pendingCheck.set(null);
            focusWhenShown(answer.executionId);
            notify("success", `Started task: ${task.label}`);
        })
        .catch(reportError(`start task ${task.label}`));
}

export function stopTaskRun(run: TaskRun): void {
    void tasksApi
        .stop(run.executionId)
        .then(() => notify("success", `Stopped ${run.label ?? run.taskId}`))
        .catch(reportError("stop task"));
}

/** What the person has going in the project, and the last run they started. */
export function personRuns(runs: readonly TaskRun[], project: string): { running: TaskRun | null; last: TaskRun | null } {
    const mine = runs.filter((run) => run.project === project && run.agentId === null);
    return { running: mine.findLast(isActive) ?? null, last: mine.at(-1) ?? null };
}
