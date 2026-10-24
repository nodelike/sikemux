import { coreCall } from "../api/core";
import { onCoreChanged } from "../api/coreChanged";
import type { PluginFailure } from "../api/plugins";
import { isSignedOutFailure } from "../plugin-api/backend";
import { resourceKind } from "../plugin-api/identity";
import { invalidate, resource } from "../state/resources";
import type { PromptContext } from "../api/acp";
import type { DiffRow } from "../api/git";
import type {
    ChangedFile,
    IssueList,
    OpenJobLog,
    OpenPull,
    OpenRun,
    PullList,
    ReleaseList,
    RepoRef,
    RunList,
    TrackedItem,
    TrackedKind,
} from "./types";

type Answer<T> = { value: T } | { failed: PluginFailure & { plugin: string } };

/** One of the core's lists of the project at `project`; a failure rejects as its plugin's would, so a sign-in gone reads as one. */
async function list<T>(method: string, params: Record<string, unknown>): Promise<T> {
    follow();
    const answer = await coreCall<Answer<T>>(method, params);
    if ("value" in answer) return answer.value;
    const { failed } = answer;
    if (isSignedOutFailure(failed)) invalidate((kind) => kind === resourceKind(failed.plugin, "status"));
    throw failed;
}

/** Each list by the name a `codeHost` change gives it. */
const LISTS: Readonly<Record<string, string>> = {
    pulls: "host.pullList",
    runs: "host.runList",
    issues: "host.issueList",
    releases: "host.releaseList",
    tracked: "host.trackedList",
    pull: "host.pullView",
    pullFiles: "host.pullFiles",
    run: "host.runView",
    jobLog: "host.jobLogView",
};
const EVERY_LIST = new Set(Object.values(LISTS));
let following = false;

/** The core says when lists moved, so they are read again then and at no other time; it answers the unchanged ones from what it keeps. */
function follow(): void {
    if (following) return;
    following = true;
    onCoreChanged("codeHost", ({ paths }) => {
        const moved = paths ? new Set(paths.map((name) => LISTS[name])) : EVERY_LIST;
        invalidate((kind) => moved.has(kind));
    });
}

/** Of the project's code host, or of `repo` when no project holds it. */
const placeOf = (project: string | null, repo: RepoRef) => (project ? { project } : { repo });

export interface RunFilter {
    workflowId?: string;
    branch?: string;
    status?: string;
    headSha?: string;
    page?: number;
    perPage?: number;
}

export const pullListR = resource({
    kind: "host.pullList",
    fetch: (project: string, state: string): Promise<PullList> => list("codeHostPulls", { project, state }),
});

export const runListR = resource({
    kind: "host.runList",
    fetch: (project: string, filter: RunFilter): Promise<RunList> => list("codeHostRuns", { project, ...filter }),
});

export const issueListR = resource({
    kind: "host.issueList",
    fetch: (project: string, state: string, page: number): Promise<IssueList> => list("codeHostIssues", { project, state, page }),
});

export const releaseListR = resource({
    kind: "host.releaseList",
    fetch: (project: string): Promise<ReleaseList> => list("codeHostReleases", { project }),
});

/** The open issues and pull requests a message can be about, newest first. */
export const trackedListR = resource({
    kind: "host.trackedList",
    fetch: (project: string): Promise<TrackedItem[]> => list("codeHostTracked", { project }),
});

/** One pull request, with its reviews' verdict, whether it can merge, and the runs on its head commit: of the project's code host, or of `repo` when no project holds it. */
export const pullViewR = resource({
    kind: "host.pullView",
    fetch: (project: string | null, repo: RepoRef, number: number): Promise<OpenPull> => list("codeHostPull", { ...placeOf(project, repo), number }),
});

/** A file a pull request changes, with its patch as the rows the git pane's diff draws, as the core reads them. */
export type PulledFile = Omit<ChangedFile, "patch"> & { rows: DiffRow[] };

/** The files a pull request changes: of the project's code host, or of `repo` when no project holds it. */
export const pullFilesR = resource({
    kind: "host.pullFiles",
    fetch: (project: string | null, repo: RepoRef, number: number): Promise<PulledFile[]> =>
        list("codeHostPullFiles", { ...placeOf(project, repo), number }),
});

/** Checks pull request `number` out in the project, answering the branch it is on. */
export async function checkoutPull(project: string, number: number): Promise<string> {
    const { branch } = await list<{ branch: string }>("codeHostCheckout", { project, number });
    return branch;
}

/** An issue or pull request of the project's repository in full, as the core writes it for an agent. */
export function loadTrackedContext(project: string, kind: TrackedKind, number: number): Promise<PromptContext> {
    return list("codeHostTrackedContext", { project, kind, number });
}

/** What a new pull request from the project starts with, as the core works it out. */
export interface NewPull {
    /** The project's branch, unless others usually merge into it. */
    head: string | null;
    /** The branch it goes into by default. */
    base: string | null;
    /** The host's branches. */
    branches: string[];
    /** The head is on the host already. */
    pushed: boolean;
}

export const newPullR = resource({
    kind: "host.newPull",
    fetch: (project: string): Promise<NewPull> => list("codeHostNewPull", { project }),
});

/** One attempt of a run, or the latest when `attempt` is null, with its jobs laid out; the core reads it again while it is going. */
export const runViewR = resource({
    kind: "host.runView",
    fetch: (project: string | null, repo: RepoRef, runId: string, attempt: number | null): Promise<OpenRun> =>
        list("codeHostRun", { ...placeOf(project, repo), runId, attempt }),
});

/** A job's log split into its steps; the core reads it again while the job is going. */
export const jobLogViewR = resource({
    kind: "host.jobLogView",
    fetch: (project: string | null, repo: RepoRef, runId: string, jobId: string): Promise<OpenJobLog> =>
        list("codeHostJobLog", { ...placeOf(project, repo), runId, jobId }),
});
