/*
 * The shapes every code host hands the Git pane: pull requests, CI runs,
 * issues, releases and the rest. A host's plugin maps its own API onto these.
 * Statuses and conclusions use GitHub's words, which other hosts map onto.
 */

export interface Repo {
    host: string;
    owner: string;
    name: string;
}

/** How much of a host's rate limit is left. A host that says nothing about it leaves the numbers out. */
export interface RateLimit {
    /** Requests are being held back until `resetsAt`. */
    limited: boolean;
    /** Seconds since the epoch. */
    resetsAt: number | null;
    remaining: number | null;
    limit: number | null;
    /** Little of the limit is left. */
    near: boolean;
}

export interface Resolved {
    repo: Repo | null;
    slug: string | null;
    sameHost: boolean;
}

export interface RepoListing {
    owner: string;
    name: string;
    slug: string;
    private: boolean;
    archived: boolean;
    defaultBranch: string | null;
    pushedAt: string | null;
    url: string;
}

/** Which repository a call is about. Every read and write carries one. */
export interface RepoRef {
    /** The code host's id, which picks the plugin that answers. */
    provider: string;
    owner: string;
    name: string;
    /** The signed-in account to ask as; with none, the host's default account. */
    account?: string | null;
}

export interface Workflow {
    id: string;
    name: string;
    path: string;
    state: string;
    active: boolean;
    url: string;
}

export type RunStatus =
    | "queued"
    | "in_progress"
    | "completed"
    | "requested"
    | "waiting"
    | "pending"
    | "success"
    | "failure"
    | "neutral"
    | "cancelled"
    | "skipped"
    | "timed_out"
    | "action_required";

export interface Run {
    id: string;
    name: string;
    title: string;
    workflowId: string;
    path: string | null;
    runNumber: number;
    attempt: number;
    event: string;
    status: string;
    conclusion: string | null;
    branch: string | null;
    sha: string;
    shortSha: string;
    actor: string | null;
    avatarUrl: string | null;
    createdAt: string;
    startedAt: string | null;
    updatedAt: string;
    pullRequests: number[];
    url: string;
}

export interface RunQuery extends RepoRef {
    workflowId?: string;
    branch?: string;
    status?: RunStatus;
    event?: string;
    actor?: string;
    headSha?: string;
    page?: number;
    perPage?: number;
}

export interface RunPage {
    runs: Run[];
    total: number;
    nextPage: number | null;
}

export interface Step {
    number: number;
    name: string;
    status: string;
    conclusion: string | null;
    startedAt: string | null;
    completedAt: string | null;
}

export interface Job {
    id: string;
    name: string;
    status: string;
    conclusion: string | null;
    startedAt: string | null;
    completedAt: string | null;
    runner: string | null;
    url: string | null;
    /** Where this job's annotations live; absent on a job GitHub never checked. */
    checkRunId: string | null;
    steps: Step[];
}

export interface RunDetail {
    run: Run;
    jobs: Job[];
}

export interface LogLine {
    number: number;
    timestamp: string | null;
    text: string;
}

export interface JobLog {
    lines: LogLine[];
    expired: boolean;
    /** The log was longer than 16 MiB, so `lines` holds only its last 16 MiB. */
    truncated: boolean;
}

export interface Annotation {
    path: string | null;
    startLine: number | null;
    endLine: number | null;
    /** `failure`, `warning` or `notice`. */
    level: string;
    title: string | null;
    message: string;
    details: string | null;
}

export interface JobSummary {
    title: string;
    body: string;
}

export interface Billable {
    runner: string;
    totalMs: number;
    jobs: number;
}

export interface RunTiming {
    runDurationMs: number | null;
    billable: Billable[];
}

export interface WorkflowFile {
    path: string;
    text: string;
}

export interface Artifact {
    id: string;
    name: string;
    sizeBytes: number;
    expired: boolean;
    createdAt: string | null;
    expiresAt: string | null;
}

export interface SavedArtifact {
    path: string;
    bytes: number;
}

/** How far a download has got. `total` is unknown when GitHub did not say. */
export interface DownloadProgress {
    received: number;
    total: number | null;
}

export interface PendingApproval {
    environmentId: number;
    environment: string;
    waitMinutes: number;
    canApprove: boolean;
    reviewers: string[];
}

/**
 * One read of a watched run. `run` and `jobs` are the last ones read, so a tick whose read failed still carries them
 * (both are empty only if no read has worked yet). `error` says why this read failed. `finished` is the last tick:
 * with no error the run is over; with one the watch gave up, at once when signed out or the run is gone, or after
 * repeated failures.
 */
export interface RunTick {
    run: Run | null;
    jobs: Job[];
    error: string | null;
    finished: boolean;
    /** On the finished tick: signed out, or the run is gone, so starting the watch again will not help. */
    fatal: boolean;
    /** The token was refused or is missing. */
    signedOut: boolean;
}

export interface Label {
    name: string;
    color: string;
}

export interface Comment {
    id: number;
    author: string | null;
    avatarUrl: string | null;
    /** `OWNER`, `MEMBER`, `COLLABORATOR`, `CONTRIBUTOR` and the rest. */
    authorAssociation: string | null;
    body: string;
    createdAt: string;
    url: string | null;
}

export type PullState = "open" | "closed" | "merged";

export interface Pull {
    number: number;
    title: string;
    body: string;
    state: string;
    draft: boolean;
    author: string | null;
    avatarUrl: string | null;
    authorAssociation: string | null;
    head: string | null;
    /** `owner:branch`, which names the fork a branch lives on. */
    headLabel: string | null;
    base: string | null;
    headSha: string | null;
    createdAt: string;
    updatedAt: string;
    /** Unknown when GitHub did not say. */
    comments: number | null;
    additions: number | null;
    deletions: number | null;
    changedFiles: number | null;
    mergeable: boolean | null;
    mergeState: string | null;
    labels: Label[];
    reviewers: string[];
    assignees: string[];
    milestone: string | null;
    commits: number | null;
    mergedAt: string | null;
    mergedBy: string | null;
    mergeCommitSha: string | null;
    /** The picture of each person named above by login alone. */
    avatars: Record<string, string>;
    url: string;
}

/** One thing in a pull request's or issue's history, in the one shape the backend gives every event. */
export interface TimelineItem {
    /** GitHub's event name, such as `commented`, `reviewed` or `committed`. */
    kind: string;
    id: number | null;
    actor: string | null;
    avatarUrl: string | null;
    association: string | null;
    at: string | null;
    body: string | null;
    /** A review's verdict, in lower case, or why an issue was closed. */
    state: string | null;
    sha: string | null;
    message: string | null;
    /** Who or what it was about: a requested reviewer, a label, a new title. */
    subject: string | null;
}

/** Who wrote a commit, by the email in it, and the account the host matched that email to. */
export interface CommitAuthor {
    email: string;
    login: string;
    avatarUrl: string;
}

export interface PullCommit {
    sha: string;
    message: string;
    author: string | null;
    avatarUrl: string | null;
    date: string | null;
}

export interface ChangedFile {
    path: string;
    status: string;
    additions: number;
    deletions: number;
    previousPath?: string;
    patch?: string;
}

export interface Review {
    author: string | null;
    avatarUrl: string | null;
    state: string;
    body: string;
    submittedAt: string | null;
}

export type MergeMethod = "merge" | "squash" | "rebase";

export type ReviewEvent = "APPROVE" | "REQUEST_CHANGES" | "COMMENT";

export interface NewPull {
    title: string;
    head: string;
    base: string;
    body: string;
    draft: boolean;
}

export interface Issue {
    number: number;
    title: string;
    body: string;
    state: string;
    /** Why it was closed: `completed`, `not_planned` or `reopened`. */
    stateReason: string | null;
    author: string | null;
    avatarUrl: string | null;
    createdAt: string;
    updatedAt: string;
    closedAt: string | null;
    comments: number;
    labels: Label[];
    assignees: string[];
    url: string;
}

export interface IssuePage {
    issues: Issue[];
    total: number;
    nextPage: number | null;
}

export interface ReleaseAsset {
    id: number;
    name: string;
    sizeBytes: number;
    downloads: number;
}

export interface Release {
    id: number;
    tag: string;
    name: string;
    body: string;
    draft: boolean;
    prerelease: boolean;
    publishedAt: string | null;
    author: string | null;
    assets: ReleaseAsset[];
    url: string;
}

export interface Notification {
    id: string;
    title: string;
    kind: string;
    reason: string;
    repo: string;
    number: number | null;
    unread: boolean;
    updatedAt: string;
    url: string | null;
}
