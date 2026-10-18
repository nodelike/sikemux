import { peekResource, resource, useResource } from "../../plugin-api/resources";
import {
    rundeckApi,
    type JobDetail,
    type JobIndexEntry,
    type MatrixCell,
    type MatrixResult,
    type OptionValue,
    type PlanRequest,
    type PlanResult,
    type ProjectExecutions,
    type RundeckExecution,
    type RundeckJob,
    type RundeckProject,
    type RundeckStatus,
} from "./api";
import { DEFAULT_BRANCH_OPTIONS } from "./shape";

export const rndStatusR = resource({
    kind: "rnd.status",
    fetch: (): Promise<RundeckStatus> => rundeckApi.status(),
    staleAfterMs: 60_000,
});

export const rndProjectsR = resource({
    kind: "rnd.projects",
    fetch: (): Promise<RundeckProject[]> => rundeckApi.projects(),
    staleAfterMs: 5 * 60_000,
});

export const rndJobsR = resource({
    kind: "rnd.jobs",
    fetch: (project: string): Promise<RundeckJob[]> => rundeckApi.jobs(project),
    staleAfterMs: 60_000,
});

export const rndJobIndexR = resource({
    kind: "rnd.jobIndex",
    fetch: (): Promise<JobIndexEntry[]> => rundeckApi.jobIndex(),
    staleAfterMs: 5 * 60_000,
});

export const rndJobDetailR = resource({
    kind: "rnd.jobDetail",
    fetch: (jobId: string): Promise<JobDetail> => rundeckApi.jobDetail(jobId),
    staleAfterMs: 60_000,
});

export const rndOptionValuesR = resource({
    kind: "rnd.optionValues",
    fetch: (url: string): Promise<OptionValue[]> => rundeckApi.optionValues(url),
    staleAfterMs: 60_000,
});

export const rndMatrixR = resource({
    kind: "rnd.matrix",
    fetch: (project: string): Promise<MatrixResult> => rundeckApi.branchesMatrix(project),
    staleAfterMs: 30_000,
});

export const rndJobCellsR = resource({
    kind: "rnd.jobCells",
    fetch: (jobs: RundeckJob[]): Promise<MatrixCell[]> => rundeckApi.jobCells(jobs),
    staleAfterMs: 30_000,
});

export const rndExecutionsR = resource({
    kind: "rnd.executions",
    fetch: (jobId: string, project: string, max: number): Promise<RundeckExecution[]> => rundeckApi.executions(jobId, project, max),
    staleAfterMs: 15_000,
});

export const rndProjectExecutionsR = resource({
    kind: "rnd.projectExecutions",
    fetch: (project: string, max: number): Promise<ProjectExecutions> => rundeckApi.projectExecutions(project, max, 0),
    staleAfterMs: 15_000,
});

export const rndPlanR = resource({
    kind: "rnd.plan",
    fetch: (request: PlanRequest): Promise<PlanResult> => rundeckApi.plan(request),
    staleAfterMs: 10_000,
});

const rndBranchOptionsR = resource({
    kind: "rnd.branchOptions",
    fetch: (): Promise<string[]> => rundeckApi.branchOptions(),
});

/** Job options that hold the git branch, as the plugin keeps them. */
export function useBranchOptions(): string[] {
    return useResource(rndBranchOptionsR).data ?? DEFAULT_BRANCH_OPTIONS;
}

export const branchOptionsNow = (): string[] => peekResource(rndBranchOptionsR) ?? DEFAULT_BRANCH_OPTIONS;
