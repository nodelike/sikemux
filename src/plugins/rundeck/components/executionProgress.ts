import type { RundeckExecution, RundeckWorkflowState } from "../api";

const FINISHED_STATES = new Set(["SUCCEEDED", "FAILED", "ABORTED", "NOT_ELIGIBLE", "SKIPPED"]);

export interface ExecutionProgress {
    completed: number;
    total: number;
    percent: number;
}

export function executionProgress(state: RundeckWorkflowState | null | undefined): ExecutionProgress | null {
    if (!state) return null;
    const total = Math.max(state.stepCount ?? 0, state.steps.length);
    if (total === 0) return null;
    const completed = Math.min(total, state.steps.filter((step) => FINISHED_STATES.has((step.executionState ?? "").toUpperCase())).length);
    return { completed, total, percent: Math.round((completed / total) * 100) };
}

export function newestExecutions(executions: RundeckExecution[]): RundeckExecution[] {
    return [...executions].sort((left, right) => executionTime(right) - executionTime(left) || right.id - left.id);
}

function executionTime(execution: RundeckExecution): number {
    const started = execution["date-started"];
    if (started?.unixtime != null) return started.unixtime;
    const parsed = Date.parse(started?.date ?? "");
    return Number.isNaN(parsed) ? 0 : parsed;
}
