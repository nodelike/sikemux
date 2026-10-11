import { createPluginBackend, isPluginFailure } from "../../plugin-api/backend";
import { invalidate } from "../../plugin-api/resources";
import type {
    Annotation,
    Artifact,
    ChangedFile,
    CodeHostApi,
    Comment,
    HostAccount,
    HostAccountEntry,
    Issue,
    IssuePage,
    JobLog,
    JobSummary,
    MergeMethod,
    NewPull,
    Notification,
    PendingApproval,
    Pull,
    PullCommit,
    RateLimit,
    Release,
    RepoListing,
    RepoRef,
    Resolved,
    Review,
    ReviewEvent,
    RunDetail,
    RunPage,
    RunQuery,
    RunTick,
    RunTiming,
    ThreadOf,
    TimelineItem,
    Workflow,
    WorkflowFile,
} from "../../plugin-api/codehost";
import { GITLAB_PLUGIN_ID } from "./kinds";

const backend = createPluginBackend(GITLAB_PLUGIN_ID);

export interface GitlabStatus {
    configured: boolean;
    /** Which signed-in account this is, `host#id`. */
    account: string | null;
    host: string | null;
    login: string;
    displayName: string | null;
    avatarUrl: string | null;
    canWriteCi: boolean;
    ok: boolean;
    authFailed: boolean;
    message: string | null;
}

export function failureMessage(error: unknown): string {
    return isPluginFailure(error) ? error.message : String(error);
}

function isSignedOut(error: unknown): boolean {
    if (!isPluginFailure(error)) return false;
    return error.category === "auth" || error.category === "unconfigured" || (error.category === "http" && error.status === 401);
}

/** A token GitLab has stopped accepting makes every cached answer stale, whichever call found out. */
function forgetSignedOut(): void {
    invalidate((kind) => kind.startsWith("host."));
}

async function call<T>(method: string, params?: unknown): Promise<T> {
    try {
        return await backend.call<T>(method, params);
    } catch (error) {
        if (isSignedOut(error)) forgetSignedOut();
        if (isPluginFailure(error, "rate-limited")) invalidate((kind) => kind === "host.rateLimit");
        throw error;
    }
}

const cannot = (what: string) => (): Promise<never> => Promise.reject(new Error(`GitLab cannot ${what} here yet`));

const IMAGES_KEPT = 400;
const images = new Map<string, Promise<string>>();

/** An avatar as a `data:` address, since the window cannot load GitLab's images itself. One that failed is tried again next time. */
function image(url: string): Promise<string> {
    const known = images.get(url);
    if (known) return known;
    const fetched = call<string>("image", { url });
    images.set(url, fetched);
    fetched.catch(() => images.delete(url));
    if (images.size > IMAGES_KEPT) {
        const oldest = images.keys().next().value;
        if (oldest !== undefined) images.delete(oldest);
    }
    return fetched;
}

export const gitlabApi = {
    status: (account: string | null = null) => backend.call<GitlabStatus>("status", { account }),
    /** An empty host signs in to gitlab.com. */
    signInWithToken: (token: string, host: string) => backend.call<GitlabStatus>("signInWithToken", { token, host }),
};

export function accountOf(status: GitlabStatus): HostAccount {
    return {
        id: status.account,
        ok: status.ok,
        login: status.login,
        avatarUrl: status.avatarUrl,
        host: status.host ?? "gitlab.com",
        canWriteCi: status.canWriteCi,
        warning: status.ok && !status.canWriteCi ? "This token has read_api only, so it cannot start, retry or cancel pipelines." : null,
    };
}

interface ListedAccount {
    id: string;
    host: string;
    login: string;
    displayName: string | null;
    avatarUrl: string | null;
    isDefault: boolean;
}

/** Each account names its server unless it is gitlab.com, since one person may be signed in on both. */
export function entryOf(account: ListedAccount): HostAccountEntry {
    const name = account.displayName && account.displayName !== account.login ? account.displayName : null;
    const server = account.host === "gitlab.com" ? null : account.host;
    return {
        id: account.id,
        login: account.login,
        detail: [name, server].filter(Boolean).join(" · ") || null,
        avatarUrl: account.avatarUrl,
        isDefault: account.isDefault,
    };
}

/** GitLab as the Git pane reads any code host. */
export const gitlabHostApi: CodeHostApi = {
    rateLimit: (account: string | null) => backend.call<RateLimit>("rateLimit", { account }),
    status: (account: string | null) => gitlabApi.status(account).then(accountOf),
    signOut: (account: string | null) => backend.call<void>("signOut", { account }),
    accounts: () => backend.call<ListedAccount[]>("accounts").then((listed) => listed.map(entryOf)),
    setDefaultAccount: (account: string) => backend.call<void>("setDefaultAccount", { id: account }),
    accountFor: (repo: RepoRef) => call<string | null>("accountFor", repo),
    resolveRemote: (url: string) => backend.call<Resolved>("resolveRemote", { url }),
    myRepos: (account: string | null, limit = 50) => call<RepoListing[]>("myRepos", { account, limit }),
    image,

    workflows: (repo: RepoRef) => call<Workflow[]>("workflows", repo),
    branches: (repo: RepoRef) => call<string[]>("branches", repo),
    runs: (query: RunQuery) => call<RunPage>("runs", query),
    run: (repo: RepoRef, runId: string) => call<RunDetail>("run", { ...repo, runId }),
    runAttempt: cannot("show an earlier attempt of a pipeline"),
    jobLog: (repo: RepoRef, jobId: string) => call<JobLog>("jobLog", { ...repo, jobId }),
    annotations: (): Promise<Annotation[]> => Promise.resolve([]),
    jobSummary: (): Promise<JobSummary | null> => Promise.resolve(null),
    runTiming: (repo: RepoRef, runId: string) => call<RunTiming>("runTiming", { ...repo, runId }),
    workflowFile: (repo: RepoRef) => call<WorkflowFile>("workflowFile", repo),
    artifacts: (): Promise<Artifact[]> => Promise.resolve([]),
    pendingApprovals: (): Promise<PendingApproval[]> => Promise.resolve([]),
    dispatch: (repo: RepoRef, workflowId: string, gitRef: string, inputs: Record<string, string>) =>
        call<void>("dispatch", { ...repo, workflowId, gitRef, inputs }),
    rerun: (repo: RepoRef, runId: string, failedOnly: boolean) => call<void>("rerun", { ...repo, runId, failedOnly }),
    rerunJob: (repo: RepoRef, jobId: string) => call<void>("rerunJob", { ...repo, jobId }),
    cancel: (repo: RepoRef, runId: string) => call<void>("cancel", { ...repo, runId }),
    deleteRunLogs: cannot("delete a pipeline's logs alone"),
    deleteRun: (repo: RepoRef, runId: string) => call<void>("deleteRun", { ...repo, runId }),
    reviewDeployment: cannot("approve a deployment"),
    downloadArtifact: cannot("download job artifacts"),
    watchStart: (repo: RepoRef, runId: string, onTick: (tick: RunTick) => void) =>
        backend
            .openStream<RunTick>("watchRun", { ...repo, runId }, (tick) => {
                if (tick.signedOut) forgetSignedOut();
                onTick(tick);
            })
            .catch((error: unknown) => {
                if (isSignedOut(error)) forgetSignedOut();
                throw error;
            }),
    watchStop: (streamId: number) => backend.closeStream(streamId),

    pulls: (repo: RepoRef, state: string) => call<Pull[]>("pulls", { ...repo, state }),
    pull: (repo: RepoRef, number: number) => call<Pull>("pull", { ...repo, number }),
    pullFiles: (repo: RepoRef, number: number) => call<ChangedFile[]>("pullFiles", { ...repo, number, fullPatches: true }),
    pullCommits: (repo: RepoRef, number: number) => call<PullCommit[]>("pullCommits", { ...repo, number }),
    pullReviews: (repo: RepoRef, number: number) => call<Review[]>("pullReviews", { ...repo, number }),
    timeline: (repo: RepoRef, number: number, of: ThreadOf = "pull") => call<TimelineItem[]>("timeline", { ...repo, number, of }),
    mergePull: (repo: RepoRef, number: number, method: MergeMethod, sha: string) => call<void>("mergePull", { ...repo, number, method, sha }),
    createPull: (repo: RepoRef, pull: NewPull) => call<Pull>("createPull", { ...repo, ...pull }),
    setPullState: (repo: RepoRef, number: number, state: "open" | "closed") => call<void>("setPullState", { ...repo, number, state }),
    reviewPull: (repo: RepoRef, number: number, event: ReviewEvent, body: string) => call<void>("reviewPull", { ...repo, number, event, body }),

    issues: (repo: RepoRef, state: string, page: number) => call<IssuePage>("issues", { ...repo, state, page }),
    issue: (repo: RepoRef, number: number) => call<Issue>("issue", { ...repo, number }),
    createIssue: (repo: RepoRef, title: string, body: string) => call<Issue>("createIssue", { ...repo, title, body }),
    setIssueState: (repo: RepoRef, number: number, state: "open" | "closed") => call<void>("setIssueState", { ...repo, number, state }),
    comments: (repo: RepoRef, number: number, of: ThreadOf = "pull") => call<Comment[]>("comments", { ...repo, number, of }),
    addComment: (repo: RepoRef, number: number, body: string, of: ThreadOf = "pull") => call<void>("addComment", { ...repo, number, body, of }),

    releases: (repo: RepoRef) => call<Release[]>("releases", repo),
    downloadAsset: cannot("download release files"),

    inbox: (account: string | null, all: boolean) => call<Notification[]>("inbox", { account, all }),
    markRead: (account: string | null, id: string) => call<void>("markRead", { account, id }),
    markAllRead: (account: string | null) => call<void>("markAllRead", { account }),
};
