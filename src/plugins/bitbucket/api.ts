import { browserSignIn, createPluginBackend, type PluginStreamHandlers } from "../../plugin-api/backend";
import type {
    Annotation,
    Artifact,
    ChangedFile,
    CommitAuthor,
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
import { createCodeHostClient } from "../../plugin-api/codehost";
import { BITBUCKET_PLUGIN_ID } from "./kinds";

export { failureMessage } from "../../plugin-api/backend";

const backend = createPluginBackend(BITBUCKET_PLUGIN_ID, { signInStatus: (kind) => kind === "host.status" || kind === "host.bitbucketStatus" });
const { call, image, watch } = createCodeHostClient(backend);

export interface BitbucketStatus {
    configured: boolean;
    /** Which signed-in account this is. */
    account: string | null;
    method: "oauth" | "token" | null;
    login: string;
    displayName: string | null;
    avatarUrl: string | null;
    canWriteCi: boolean;
    ok: boolean;
    authFailed: boolean;
    message: string | null;
    /** A build without the OAuth secret can only take a pasted token. */
    browserSignIn: boolean;
}

const cannot = (what: string) => (): Promise<never> => Promise.reject(new Error(`Bitbucket cannot ${what}`));

export const bitbucketApi = {
    status: (account: string | null = null) => backend.call<BitbucketStatus>("status", { account }),
    signInWithToken: (token: string, email: string | null) => backend.call<BitbucketStatus>("signInWithToken", { token, email }),
    /** Waits for the browser to come back from Bitbucket's sign-in page, which `openPage` is handed to show. */
    signInWithBrowser: (openPage: (url: string) => void) => browserSignIn<BitbucketStatus>(backend, "signInWithBrowser", openPage),
};

function accountOf(status: BitbucketStatus): HostAccount {
    return {
        id: status.account,
        ok: status.ok,
        login: status.login,
        avatarUrl: status.avatarUrl,
        host: "bitbucket.org",
        canWriteCi: status.canWriteCi,
        warning: status.ok && !status.canWriteCi ? "This sign-in cannot start or stop pipelines." : null,
    };
}

/** Bitbucket as the Git pane reads any code host. What it has no counterpart for is empty, or refused if it is a change. */
interface ListedAccount {
    id: string;
    login: string;
    displayName: string | null;
    avatarUrl: string | null;
    isDefault: boolean;
}

function entryOf(account: ListedAccount): HostAccountEntry {
    return {
        id: account.id,
        login: account.login,
        detail: account.displayName && account.displayName !== account.login ? account.displayName : null,
        avatarUrl: account.avatarUrl,
        isDefault: account.isDefault,
    };
}

export const bitbucketHostApi: CodeHostApi = {
    rateLimit: (account: string | null) => backend.call<RateLimit>("rateLimit", { account }),
    status: (account: string | null) => bitbucketApi.status(account).then(accountOf),
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
    rerunJob: cannot("re-run a single step"),
    cancel: (repo: RepoRef, runId: string) => call<void>("cancel", { ...repo, runId }),
    deleteRunLogs: cannot("delete a pipeline's logs"),
    deleteRun: cannot("delete a pipeline"),
    reviewDeployment: cannot("approve a deployment"),
    downloadArtifact: cannot("download pipeline artifacts"),
    watch: (repo: RepoRef, runId: string, handlers: PluginStreamHandlers<RunTick>) => watch<RunTick>({ ...repo, runId }, handlers),

    pulls: (repo: RepoRef, state: string) => call<Pull[]>("pulls", { ...repo, state }),
    pull: (repo: RepoRef, number: number) => call<Pull>("pull", { ...repo, number }),
    pullFiles: (repo: RepoRef, number: number) => call<ChangedFile[]>("pullFiles", { ...repo, number, fullPatches: true }),
    pullCommits: (repo: RepoRef, number: number) => call<PullCommit[]>("pullCommits", { ...repo, number }),
    commitAuthors: (repo: RepoRef, gitRef: string | null) => call<CommitAuthor[]>("commitAuthors", { ...repo, gitRef }),
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
