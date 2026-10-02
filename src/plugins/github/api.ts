import { createPluginBackend, isPluginFailure } from "../../plugin-api/backend";
import { invalidate } from "../../plugin-api/resources";
import type {
    Annotation,
    Artifact,
    ChangedFile,
    Comment,
    DownloadProgress,
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
    Job,
    Release,
    RepoListing,
    RepoRef,
    Resolved,
    Review,
    ReviewEvent,
    Run,
    RunDetail,
    RunPage,
    RunQuery,
    RunTick,
    RunTiming,
    SavedArtifact,
    TimelineItem,
    Workflow,
    WorkflowFile,
    CodeHostApi,
    CommitAuthor,
    HostAccount,
    HostAccountEntry,
} from "../../plugin-api/codehost";
import { GITHUB_PLUGIN_ID } from "./kinds";

export type * from "../../plugin-api/codehost";

const backend = createPluginBackend(GITHUB_PLUGIN_ID);

export type TokenSource = "keychain" | "environment" | "ghCli";

export interface ActionsStatus {
    configured: boolean;
    /** Which signed-in account this is; null while a borrowed token stands in before anyone has signed in. */
    account: string | null;
    host: string;
    login: string;
    tokenSource: TokenSource | null;
    /** The variable an environment token was read from. */
    tokenVariable: string | null;
    scopes: string[];
    canWriteWorkflows: boolean;
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

/** A token GitHub has stopped accepting makes every cached answer stale, whichever call found out, so the next read lands on the sign-in form. */
function forgetSignedOut(): void {
    invalidate((kind) => kind.startsWith("host."));
}

/** GitHub numbers its runs, jobs, workflows and artifacts, and the Git pane names them by text, as every host can. */
interface GithubRun extends Omit<Run, "id" | "workflowId"> {
    id: number;
    workflowId: number;
}

interface GithubJob extends Omit<Job, "id" | "checkRunId"> {
    id: number;
    checkRunId: number | null;
}

interface GithubRunDetail {
    run: GithubRun;
    jobs: GithubJob[];
}

interface GithubRunPage extends Omit<RunPage, "runs"> {
    runs: GithubRun[];
}

interface GithubRunTick extends Omit<RunTick, "run" | "jobs"> {
    run: GithubRun | null;
    jobs: GithubJob[];
}

const fromRun = (run: GithubRun): Run => ({ ...run, id: String(run.id), workflowId: String(run.workflowId) });
const fromJob = (job: GithubJob): Job => ({ ...job, id: String(job.id), checkRunId: job.checkRunId === null ? null : String(job.checkRunId) });
const fromDetail = (detail: GithubRunDetail): RunDetail => ({ run: fromRun(detail.run), jobs: detail.jobs.map(fromJob) });
const withTextId = <T extends { id: number }>(item: T): Omit<T, "id"> & { id: string } => ({ ...item, id: String(item.id) });

async function call<T>(method: string, params?: unknown): Promise<T> {
    try {
        return await backend.call<T>(method, params);
    } catch (error) {
        if (isSignedOut(error)) forgetSignedOut();
        if (isPluginFailure(error, "rate-limited")) invalidate((kind) => kind === "host.rateLimit");
        throw error;
    }
}

const IMAGES_KEPT = 400;
const images = new Map<string, Promise<string>>();

/**
 * An avatar or a picture from GitHub as a `data:` address, since the window cannot load GitHub's images itself. Each
 * address is fetched once; one that failed is tried again next time.
 */
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

interface DownloadTick extends DownloadProgress {
    saved: SavedArtifact | null;
}

/** Files can take longer than any single call may, so they arrive as a stream of progress that ends where the file was saved. */
function download(method: string, params: unknown, onProgress?: (progress: DownloadProgress) => void): Promise<SavedArtifact> {
    return new Promise((resolve, reject) => {
        let saved: SavedArtifact | null = null;
        backend.stream<DownloadTick>(method, params, {
            onItem: (tick) => {
                if (tick.saved) saved = tick.saved;
                else onProgress?.(tick);
            },
            onEnd: () => (saved ? resolve(saved) : reject(new Error("the download ended without saving anything"))),
            onError: (error) => {
                if (isSignedOut(error)) forgetSignedOut();
                reject(error);
            },
        });
    });
}

export const actionsApi = {
    status: (account: string | null = null) => backend.call<ActionsStatus>("status", { account }),
    signIn: (host: string, token?: string) => backend.call<ActionsStatus>("signIn", { host, token }),
    signOut: (account: string | null) => backend.call<void>("signOut", { account }),

    resolveRemote: (url: string) => backend.call<Resolved>("resolveRemote", { url }),
    myRepos: (account: string | null, limit = 50) => call<RepoListing[]>("myRepos", { account, limit }),

    workflows: (repo: RepoRef) => call<(Omit<Workflow, "id"> & { id: number })[]>("workflows", repo).then((rows): Workflow[] => rows.map(withTextId)),
    branches: (repo: RepoRef) => call<string[]>("branches", repo),
    runs: (query: RunQuery) =>
        call<GithubRunPage>("runs", { ...query, workflowId: query.workflowId === undefined ? undefined : Number(query.workflowId) }).then(
            (page): RunPage => ({ ...page, runs: page.runs.map(fromRun) }),
        ),
    run: (repo: RepoRef, runId: string) => call<GithubRunDetail>("run", { ...repo, runId: Number(runId) }).then(fromDetail),
    jobLog: (repo: RepoRef, jobId: string) => call<JobLog>("jobLog", { ...repo, jobId: Number(jobId) }),
    annotations: (repo: RepoRef, checkRunId: string) => call<Annotation[]>("annotations", { ...repo, checkRunId: Number(checkRunId) }),
    jobSummary: (repo: RepoRef, checkRunId: string) => call<JobSummary | null>("jobSummary", { ...repo, checkRunId: Number(checkRunId) }),
    runTiming: (repo: RepoRef, runId: string) => call<RunTiming>("runTiming", { ...repo, runId: Number(runId) }),
    workflowFile: (repo: RepoRef, workflowId: string) => call<WorkflowFile>("workflowFile", { ...repo, workflowId: Number(workflowId) }),
    artifacts: (repo: RepoRef, runId: string) =>
        call<(Omit<Artifact, "id"> & { id: number })[]>("artifacts", { ...repo, runId: Number(runId) }).then((rows): Artifact[] =>
            rows.map(withTextId),
        ),
    pendingApprovals: (repo: RepoRef, runId: string) => call<PendingApproval[]>("pendingApprovals", { ...repo, runId: Number(runId) }),
    runAttempt: (repo: RepoRef, runId: string, attempt: number) =>
        call<GithubRunDetail>("runAttempt", { ...repo, runId: Number(runId), attempt }).then(fromDetail),

    pulls: (repo: RepoRef, state: string) => call<Pull[]>("pulls", { ...repo, state }),
    pull: (repo: RepoRef, number: number) => call<Pull>("pull", { ...repo, number }),
    pullFiles: (repo: RepoRef, number: number) => call<ChangedFile[]>("pullFiles", { ...repo, number, fullPatches: true }),
    pullCommits: (repo: RepoRef, number: number) => call<PullCommit[]>("pullCommits", { ...repo, number }),
    timeline: (repo: RepoRef, number: number) => call<TimelineItem[]>("timeline", { ...repo, number }),
    commitAuthors: (repo: RepoRef, gitRef: string | null) => call<CommitAuthor[]>("commitAuthors", { ...repo, gitRef }),
    pullReviews: (repo: RepoRef, number: number) => call<Review[]>("pullReviews", { ...repo, number }),
    issues: (repo: RepoRef, state: string, page: number) => call<IssuePage>("issues", { ...repo, state, page }),
    issue: (repo: RepoRef, number: number) => call<Issue>("issue", { ...repo, number }),
    comments: (repo: RepoRef, number: number) => call<Comment[]>("comments", { ...repo, number }),
    releases: (repo: RepoRef) => call<Release[]>("releases", repo),
    inbox: (account: string | null, all: boolean) => call<Notification[]>("inbox", { account, all }),
    image,

    /** `sha` is the head commit the person saw; GitHub refuses the merge if the branch has moved since. */
    mergePull: (repo: RepoRef, number: number, method: MergeMethod, sha: string) => call<void>("mergePull", { ...repo, number, method, sha }),
    createPull: (repo: RepoRef, pull: NewPull) => call<Pull>("createPull", { ...repo, ...pull }),
    setPullState: (repo: RepoRef, number: number, state: "open" | "closed") => call<void>("setPullState", { ...repo, number, state }),
    reviewPull: (repo: RepoRef, number: number, event: ReviewEvent, body: string) => call<void>("reviewPull", { ...repo, number, event, body }),
    createIssue: (repo: RepoRef, title: string, body: string) => call<Issue>("createIssue", { ...repo, title, body }),
    setIssueState: (repo: RepoRef, number: number, state: "open" | "closed") => call<void>("setIssueState", { ...repo, number, state }),
    addComment: (repo: RepoRef, number: number, body: string) => call<void>("addComment", { ...repo, number, body }),
    downloadAsset: (repo: RepoRef, assetId: number, name: string, onProgress?: (progress: DownloadProgress) => void) =>
        download("downloadAsset", { ...repo, assetId, fileName: name }, onProgress),
    markRead: (account: string | null, id: string) => call<void>("markRead", { account, id }),
    markAllRead: (account: string | null) => call<void>("markAllRead", { account }),

    dispatch: (repo: RepoRef, workflowId: string, gitRef: string, inputs: Record<string, string>) =>
        call<void>("dispatch", { ...repo, workflowId: Number(workflowId), gitRef, inputs }),
    rerun: (repo: RepoRef, runId: string, failedOnly: boolean, debug = false) =>
        call<void>("rerun", { ...repo, runId: Number(runId), failedOnly, debug }),
    rerunJob: (repo: RepoRef, jobId: string, debug = false) => call<void>("rerunJob", { ...repo, jobId: Number(jobId), debug }),
    cancel: (repo: RepoRef, runId: string) => call<void>("cancel", { ...repo, runId: Number(runId) }),
    deleteRunLogs: (repo: RepoRef, runId: string) => call<void>("deleteRunLogs", { ...repo, runId: Number(runId) }),
    deleteRun: (repo: RepoRef, runId: string) => call<void>("deleteRun", { ...repo, runId: Number(runId) }),
    downloadArtifact: (repo: RepoRef, artifactId: string, name: string, onProgress?: (progress: DownloadProgress) => void) =>
        download("downloadArtifact", { ...repo, artifactId: Number(artifactId), fileName: name }, onProgress),
    reviewDeployment: (repo: RepoRef, runId: string, environmentIds: number[], state: "approved" | "rejected", comment = "") =>
        call<void>("reviewDeployment", { ...repo, runId: Number(runId), environmentIds, state, comment }),

    watchStart: (repo: RepoRef, runId: string, onTick: (tick: RunTick) => void) =>
        backend
            .openStream<GithubRunTick>("watchRun", { ...repo, runId: Number(runId) }, (tick) => {
                if (tick.signedOut) forgetSignedOut();
                onTick({ ...tick, run: tick.run && fromRun(tick.run), jobs: tick.jobs.map(fromJob) });
            })
            .catch((error: unknown) => {
                if (isSignedOut(error)) forgetSignedOut();
                throw error;
            }),
    watchStop: (streamId: number) => backend.closeStream(streamId),
};

/** GitHub serves every avatar from one place, and a company's own GitHub serves its users' from itself. */
function avatarOf(login: string, host: string): string {
    return host === "github.com" ? `https://avatars.githubusercontent.com/${login}?s=64` : `https://${host}/${login}.png?size=64`;
}

/** Where the signed-in account lives, so a name alone can find its picture. */
let accountHost = "github.com";

export function avatarForLogin(login: string): string | null {
    return login.endsWith("[bot]") || login.includes("/") ? null : avatarOf(login, accountHost);
}

function accountOf(status: ActionsStatus): HostAccount {
    if (status.host) accountHost = status.host;
    return {
        id: status.account,
        ok: status.ok,
        login: status.login,
        avatarUrl: status.login ? avatarOf(status.login, status.host) : null,
        host: status.host,
        canWriteCi: status.canWriteWorkflows,
        warning: status.ok && !status.canWriteWorkflows ? "This token cannot start or re-run workflows. It is missing the workflow scope." : null,
    };
}

/** GitHub as the git pane reads any code host. */
interface ListedAccount {
    id: string;
    host: string;
    login: string;
    isDefault: boolean;
}

function entryOf(account: ListedAccount): HostAccountEntry {
    return {
        id: account.id,
        login: account.login,
        detail: account.host === "github.com" ? null : account.host,
        avatarUrl: avatarOf(account.login, account.host),
        isDefault: account.isDefault,
    };
}

export const githubHostApi: CodeHostApi = {
    ...actionsApi,
    rateLimit: (account: string | null) => backend.call<RateLimit>("rateLimit", { account }),
    status: (account: string | null) => actionsApi.status(account).then(accountOf),
    accounts: () => backend.call<ListedAccount[]>("accounts").then((listed) => listed.map(entryOf)),
    setDefaultAccount: (account: string) => backend.call<void>("setDefaultAccount", { id: account }),
    accountFor: (repo: RepoRef) => call<string | null>("accountFor", repo),
};

const NOREPLY = /^(\d+)\+[^@]+@users\.noreply\.github\.com$/iu;

/** GitHub's private commit emails carry the account's number, which is all its avatar address needs. */
export function avatarForEmail(email: string): string | null {
    const found = NOREPLY.exec(email.trim());
    return found ? `https://avatars.githubusercontent.com/u/${found[1]}?s=64` : null;
}
