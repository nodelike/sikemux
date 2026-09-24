import { resource } from "../../plugin-api/resources";
import {
    rundeckApi,
    type JobDetail,
    type JobIndexEntry,
    type MatrixCell,
    type MatrixResult,
    type OptionValue,
    type PlanRequest,
    type PlanResult,
    type RundeckExecution,
    type RundeckJob,
    type RundeckProject,
    type RundeckStatus,
} from "./api";

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
    fetch: (project: string, branchOptions: string[]): Promise<MatrixResult> => rundeckApi.branchesMatrix(project, branchOptions),
    staleAfterMs: 30_000,
});

export const rndJobCellsR = resource({
    kind: "rnd.jobCells",
    fetch: (jobs: RundeckJob[], branchOptions: string[]): Promise<MatrixCell[]> => rundeckApi.jobCells(jobs, branchOptions),
    staleAfterMs: 30_000,
});

export const rndExecutionsR = resource({
    kind: "rnd.executions",
    fetch: (jobId: string, project: string, max: number): Promise<RundeckExecution[]> => rundeckApi.executions(jobId, project, max),
    staleAfterMs: 15_000,
});

export const rndPlanR = resource({
    kind: "rnd.plan",
    fetch: (request: PlanRequest): Promise<PlanResult> => rundeckApi.plan(request),
    staleAfterMs: 10_000,
});
