import { resource } from "../plugin-api/resources";
import {
    type Annotation,
    type JobSummary,
    type WorkflowFile,
    type Issue,
    type Notification,
    type PullCommit,
    type CommitAuthor,
    type TimelineItem,
    type Artifact,
    type RepoListing,
    type RepoRef,
    type RateLimit,
    type Workflow,
} from "./types";
import { hostApi, type HostAccount, type HostAccountEntry, type ThreadOf } from "./registry";

export const hostStatusR = resource({
    kind: "host.status",
    fetch: (provider: string, account: string | null): Promise<HostAccount> => hostApi(provider).status(account),
    staleAfterMs: 60_000,
});

export const rateLimitR = resource({
    kind: "host.rateLimit",
    fetch: (provider: string, account: string | null): Promise<RateLimit> => hostApi(provider).rateLimit(account),
    staleAfterMs: 30_000,
});

export const accountsR = resource({
    kind: "host.accounts",
    fetch: (provider: string): Promise<HostAccountEntry[]> => hostApi(provider).accounts(),
    staleAfterMs: 5 * 60_000,
});

export const accountForR = resource({
    kind: "host.accountFor",
    fetch: (repo: RepoRef): Promise<string | null> => hostApi(repo.provider).accountFor(repo),
    staleAfterMs: 10 * 60_000,
});

export const myReposR = resource({
    kind: "host.myRepos",
    fetch: (provider: string, account: string | null): Promise<RepoListing[]> => hostApi(provider).myRepos(account),
    staleAfterMs: 5 * 60_000,
});

export const workflowsR = resource({
    kind: "host.workflows",
    fetch: (repo: RepoRef): Promise<Workflow[]> => hostApi(repo.provider).workflows(repo),
    staleAfterMs: 5 * 60_000,
});

export const hostBranchesR = resource({
    kind: "host.branches",
    fetch: (repo: RepoRef): Promise<string[]> => hostApi(repo.provider).branches(repo),
    staleAfterMs: 5 * 60_000,
});

export const annotationsR = resource({
    kind: "host.annotations",
    fetch: (repo: RepoRef, checkRunId: string): Promise<Annotation[]> => hostApi(repo.provider).annotations(repo, checkRunId),
    staleAfterMs: 60_000,
});

export const jobSummaryR = resource({
    kind: "host.jobSummary",
    fetch: (repo: RepoRef, checkRunId: string): Promise<JobSummary | null> => hostApi(repo.provider).jobSummary(repo, checkRunId),
    staleAfterMs: 5 * 60_000,
});

export const workflowFileR = resource({
    kind: "host.workflowFile",
    fetch: (repo: RepoRef, workflowId: string): Promise<WorkflowFile> => hostApi(repo.provider).workflowFile(repo, workflowId),
    staleAfterMs: 10 * 60_000,
});

export const artifactsR = resource({
    kind: "host.artifacts",
    fetch: (repo: RepoRef, runId: string): Promise<Artifact[]> => hostApi(repo.provider).artifacts(repo, runId),
    staleAfterMs: 60_000,
});

export const timelineR = resource({
    kind: "host.timeline",
    fetch: (repo: RepoRef, number: number, of: ThreadOf): Promise<TimelineItem[]> => hostApi(repo.provider).timeline(repo, number, of),
    staleAfterMs: 30_000,
});

export const pullCommitsR = resource({
    kind: "host.pullCommits",
    fetch: (repo: RepoRef, number: number): Promise<PullCommit[]> => hostApi(repo.provider).pullCommits(repo, number),
    staleAfterMs: 60_000,
});

export const commitAuthorsR = resource({
    kind: "host.commitAuthors",
    fetch: (repo: RepoRef, gitRef: string | null): Promise<CommitAuthor[]> =>
        hostApi(repo.provider).commitAuthors?.(repo, gitRef) ?? Promise.resolve([]),
    staleAfterMs: 10 * 60_000,
});

export const issueR = resource({
    kind: "host.issue",
    fetch: (repo: RepoRef, number: number): Promise<Issue> => hostApi(repo.provider).issue(repo, number),
    staleAfterMs: 30_000,
});

export const inboxR = resource({
    kind: "host.inbox",
    fetch: (provider: string, account: string | null, all: boolean): Promise<Notification[]> => hostApi(provider).inbox(account, all),
    staleAfterMs: 30_000,
});
