import { createPluginBackend, isPluginFailure } from "../../plugin-api/backend";
import { invalidate } from "../../plugin-api/resources";
import type {
    Annotation,
    Artifact,
    ChangedFile,
    CommitAuthor,
    CodeHostApi,
    Comment,
    HostAccount,
    HostAccountEntry,
    JobLog,
    JobSummary,
    MergeMethod,
    NewPull,
    PendingApproval,
    Pull,
    PullCommit,
    RateLimit,
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
    TimelineItem,
    Workflow,
    WorkflowFile,
} from "../../plugin-api/codehost";
import { BITBUCKET_PLUGIN_ID } from "./kinds";

const backend = createPluginBackend(BITBUCKET_PLUGIN_ID);

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

export function failureMessage(error: unknown): string {
    return isPluginFailure(error) ? error.message : String(error);
}

function isSignedOut(error: unknown): boolean {
    if (!isPluginFailure(error)) return false;
    return error.category === "auth" || error.category === "unconfigured" || (error.category === "http" && error.status === 401);
}

/** A sign-in Bitbucket has stopped accepting makes every cached answer stale, whichever call found out, so the next read lands on the sign-in form. */
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

const cannot = (what: string) => (): Promise<never> => Promise.reject(new Error(`Bitbucket cannot ${what}`));

const IMAGES_KEPT = 400;
const images = new Map<string, Promise<string>>();

/** An avatar as a `data:` address, since the window cannot load Bitbucket's images itself. One that failed is tried again next time. */
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

interface BrowserSignInItem extends Partial<BitbucketStatus> {
    url?: string;
}

export interface BrowserSignIn {
    /** Resolves with the account once the browser comes back, and rejects if it never does. */
    done: Promise<BitbucketStatus>;
    cancel(): void;
}

/** Waits for the browser to come back from Bitbucket's sign-in page, which `openPage` is handed to show. */
function signInWithBrowser(openPage: (url: string) => void): BrowserSignIn {
    let cancel = () => {};
    const done = new Promise<BitbucketStatus>((resolve, reject) => {
        let status: BitbucketStatus | null = null;
        const stream = backend.stream<BrowserSignInItem>(
            "signInWithBrowser",
            {},
            {
                onItem: (item) => {
                    if (item.url) openPage(item.url);
                    else status = item as BitbucketStatus;
                },
                onEnd: () => (status ? resolve(status) : reject(new Error("the sign-in ended without an account"))),
                onError: reject,
            },
        );
        cancel = () => {
            stream.stop();
            reject(new Error("cancelled"));
        };
    });
    return { done, cancel: () => cancel() };
}

export const bitbucketApi = {
    status: (account: string | null = null) => backend.call<BitbucketStatus>("status", { account }),
    signInWithToken: (token: string, email: string | null) => backend.call<BitbucketStatus>("signInWithToken", { token, email }),
    signInWithBrowser,
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
    commitAuthors: (repo: RepoRef, gitRef: string | null) => call<CommitAuthor[]>("commitAuthors", { ...repo, gitRef }),
    pullReviews: (repo: RepoRef, number: number) => call<Review[]>("pullReviews", { ...repo, number }),
    timeline: (repo: RepoRef, number: number) => call<TimelineItem[]>("timeline", { ...repo, number }),
    mergePull: (repo: RepoRef, number: number, method: MergeMethod, sha: string) => call<void>("mergePull", { ...repo, number, method, sha }),
    createPull: (repo: RepoRef, pull: NewPull) => call<Pull>("createPull", { ...repo, ...pull }),
    setPullState: (repo: RepoRef, number: number, state: "open" | "closed") => call<void>("setPullState", { ...repo, number, state }),
    reviewPull: (repo: RepoRef, number: number, event: ReviewEvent, body: string) => call<void>("reviewPull", { ...repo, number, event, body }),

    issues: cannot("show issues here; they live in Jira"),
    issue: cannot("show issues here; they live in Jira"),
    createIssue: cannot("open issues here; they live in Jira"),
    setIssueState: cannot("change issues here; they live in Jira"),
    comments: (repo: RepoRef, number: number) => call<Comment[]>("comments", { ...repo, number }),
    addComment: (repo: RepoRef, number: number, body: string) => call<void>("addComment", { ...repo, number, body }),

    releases: cannot("show releases"),
    downloadAsset: cannot("download release files"),

    inbox: cannot("show notifications"),
    markRead: cannot("show notifications"),
    markAllRead: cannot("show notifications"),
};
