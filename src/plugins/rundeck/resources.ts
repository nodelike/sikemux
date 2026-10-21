import { peekResource, useResource } from "../../plugin-api/resources";
import { defineService } from "../../plugin-api/service";
import { rundeckApi, type PlanRequest, type RundeckJob } from "./api";
import { RUNDECK_PLUGIN } from "./kinds";
import { DEFAULT_BRANCH_OPTIONS, DEFAULT_PROD_ENVS } from "./shape";

export const rundeck = defineService(RUNDECK_PLUGIN, rundeckApi.status);

export const rndProjectsR = rundeck.resource("projects", () => rundeckApi.projects(), 5 * 60_000);

export const rndJobsR = rundeck.resource("jobs", (project: string) => rundeckApi.jobs(project), 60_000);

export const rndJobIndexR = rundeck.resource("jobIndex", () => rundeckApi.jobIndex(), 5 * 60_000);

export const rndJobDetailR = rundeck.resource("jobDetail", (jobId: string) => rundeckApi.jobDetail(jobId), 60_000);

export const rndOptionValuesR = rundeck.resource("optionValues", (url: string) => rundeckApi.optionValues(url), 60_000);

export const rndMatrixR = rundeck.resource("matrix", (project: string) => rundeckApi.branchesMatrix(project), 30_000);

export const rndJobCellsR = rundeck.resource("jobCells", (jobs: RundeckJob[]) => rundeckApi.jobCells(jobs), 30_000);

export const rndExecutionsR = rundeck.resource(
    "executions",
    (jobId: string, project: string, max: number) => rundeckApi.executions(jobId, project, max),
    15_000,
);

export const rndProjectExecutionsR = rundeck.resource(
    "projectExecutions",
    (project: string, max: number) => rundeckApi.projectExecutions(project, max, 0),
    15_000,
);

export const rndPlanR = rundeck.resource("plan", (request: PlanRequest) => rundeckApi.plan(request), 10_000);

export const rndListsR = rundeck.resource("lists", () => rundeckApi.lists());

/** Job options that hold the git branch, as the plugin keeps them. */
export function useBranchOptions(): string[] {
    return useResource(rndListsR).data?.branchOptions ?? DEFAULT_BRANCH_OPTIONS;
}

/** Words that mark a job's top folder or project as production, as the plugin keeps them. */
export function useProdEnvs(): string[] {
    return useResource(rndListsR).data?.prodEnvs ?? DEFAULT_PROD_ENVS;
}

export const branchOptionsNow = (): string[] => peekResource(rndListsR)?.branchOptions ?? DEFAULT_BRANCH_OPTIONS;
