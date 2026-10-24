import { useResource } from "../../plugin-api/resources";
import { defineService } from "../../plugin-api/service";
import { rundeckApi } from "./api";
import { RUNDECK_PLUGIN } from "./kinds";
import type {
    RundeckActivityPage,
    RundeckDeployForm,
    RundeckDeployTarget,
    RundeckJobCatalog,
    RundeckExecutionPage,
    RundeckJobPage,
    RundeckJobs,
    RundeckOptionChoice,
    RundeckPlan,
    RundeckTree,
} from "./types.generated";
import { DEFAULT_BRANCH_OPTIONS, DEFAULT_PROD_ENVS } from "./shape";

export const rundeck = defineService(RUNDECK_PLUGIN, rundeckApi.status);

/** The values an option may take, read from its values URL and kept by the core. */
export const rndOptionValuesV = rundeck.view<[url: string], RundeckOptionChoice[]>("optionValues", (url) => ({ url }));

/** The projects' folders of jobs, kept by the core for the pane's tree. */
export const rndTreeV = rundeck.view<[], RundeckTree>("treeView", () => ({}));

/** A folder's jobs with how each last ran, kept by the core; `group` empty for the project's root. */
export const rndJobsV = rundeck.view<[project: string, group: string], RundeckJobs>("jobsView", (project, group) => ({
    project,
    group: group || undefined,
}));

/** A job with its recent runs, kept by the core and read every few seconds while one goes on. */
export const rndJobV = rundeck.view<[jobId: string, project: string], RundeckJobPage>("jobView", (jobId, project) => ({ jobId, project }));

/** A project's newest `max` runs of every job, kept by the core. */
export const rndActivityV = rundeck.view<[project: string, max: number], RundeckActivityPage>("activityView", (project, max) => ({
    project,
    max,
}));

/** One run of a job, step by step, read every few seconds while it runs. */
export const rndExecutionV = rundeck.view<[executionId: number, jobId: string, project: string], RundeckExecutionPage>(
    "executionView",
    (executionId, jobId, project) => ({ executionId, jobId, project }),
);

/** The jobs named after a project's folder, the one picked to deploy with, and how it last ran. */
export const rndDeployTargetV = rundeck.view<[folder: string, picked: { project: string; jobId: string } | null], RundeckDeployTarget>(
    "deployTargetView",
    (folder, picked) => ({ folder, picked }),
);

/** Every job in every project, to search. */
export const rndCatalogV = rundeck.view<[], RundeckJobCatalog>("catalogView", () => ({}));

/** The form that starts a job, with the options an earlier run was given. */
export const rndDeployV = rundeck.view<
    [jobId: string, project: string, name: string, group: string, branch: string, given: Record<string, string>],
    RundeckDeployForm
>("deployView", (jobId, project, name, group, branch, given) => ({
    jobId,
    project,
    name,
    group: group || undefined,
    branch: branch || undefined,
    given,
}));

/** What deploying `targetBranch` would do, checked against the linked checkout at `repoPath`. */
export const rndPlanV = rundeck.view<[jobId: string, project: string, service: string, targetBranch: string, repoPath: string], RundeckPlan>(
    "planView",
    (jobId, project, service, targetBranch, repoPath) => ({ jobId, project, service, targetBranch, repoPath }),
);

export const rndListsR = rundeck.resource("lists", () => rundeckApi.lists());

/** Job options that hold the git branch, as the plugin keeps them. */
export function useBranchOptions(): string[] {
    return useResource(rndListsR).data?.branchOptions ?? DEFAULT_BRANCH_OPTIONS;
}

/** Words that mark a job's top folder or project as production, as the plugin keeps them. */
export function useProdEnvs(): string[] {
    return useResource(rndListsR).data?.prodEnvs ?? DEFAULT_PROD_ENVS;
}
