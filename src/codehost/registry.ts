import { createContext, useContext, type ComponentType, type ReactNode } from "react";
import { isPluginEnabled } from "../plugins/enabled";
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
    CommitAuthor,
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
    SavedArtifact,
    TimelineItem,
    Workflow,
    WorkflowFile,
} from "./types";

/** Who is signed in to a host, in the terms every host shares. */
export interface HostAccount {
    /** Which signed-in account this is; null while a borrowed token stands in before anyone has signed in. */
    id: string | null;
    /** Signed in and the token works. */
    ok: boolean;
    login: string;
    avatarUrl: string | null;
    /** The server, such as github.com or a company's own. */
    host: string;
    /** Whether this account may start, re-run and cancel CI. */
    canWriteCi: boolean;
    /** Something to tell the person about the sign-in, such as a missing permission. */
    warning: string | null;
}

/** One of the accounts signed in to a host. */
export interface HostAccountEntry {
    id: string;
    login: string;
    /** What tells two accounts apart beyond their login, such as the server or a full name. */
    detail: string | null;
    avatarUrl: string | null;
    /** The account a project uses when nothing else chooses one. */
    isDefault: boolean;
}

/** What a host supports. The Git pane leaves out whatever a host cannot do rather than showing it empty. */
export interface HostCapabilities {
    ci: {
        graph: boolean;
        attempts: boolean;
        approvals: boolean;
        dispatch: boolean;
        annotations: boolean;
        summaries: boolean;
        artifacts: boolean;
        billing: boolean;
        workflowFile: boolean;
        rerunFailed: boolean;
        rerunJob: boolean;
        debugLogs: boolean;
        deleteRuns: boolean;
    };
    pulls: {
        draft: boolean;
        mergeMethods: readonly MergeMethod[];
        requestChanges: boolean;
        reopen: boolean;
        /** Whether the host says up front if a pull request would merge cleanly. */
        mergeability: boolean;
    };
    issues: boolean;
    releases: boolean;
    inbox: boolean;
}

/** Whether a thread belongs to a pull request or an issue. GitHub numbers both in one sequence; GitLab does not. */
export type ThreadOf = "pull" | "issue";

/** Every read and write a host answers. Each takes the repository it is about, whose `provider` names the host. */
export interface CodeHostApi {
    /** Every call below that takes no repository is about `account`, or the default account when it is null. */
    status(account: string | null): Promise<HostAccount>;
    signOut(account: string | null): Promise<void>;
    rateLimit(account: string | null): Promise<RateLimit>;
    accounts(): Promise<HostAccountEntry[]>;
    setDefaultAccount(account: string): Promise<void>;
    /** The first signed-in account, default first, that can see the repository. */
    accountFor(repo: RepoRef): Promise<string | null>;
    /** The repository a git remote points at, and in `sameHost` whether it is on the server this host talks to. */
    resolveRemote(url: string): Promise<Resolved>;
    myRepos(account: string | null, limit?: number): Promise<RepoListing[]>;
    image(url: string): Promise<string>;

    workflows(repo: RepoRef): Promise<Workflow[]>;
    branches(repo: RepoRef): Promise<string[]>;
    runs(query: RunQuery): Promise<RunPage>;
    run(repo: RepoRef, runId: string): Promise<RunDetail>;
    runAttempt(repo: RepoRef, runId: string, attempt: number): Promise<RunDetail>;
    jobLog(repo: RepoRef, jobId: string): Promise<JobLog>;
    annotations(repo: RepoRef, checkRunId: string): Promise<Annotation[]>;
    jobSummary(repo: RepoRef, checkRunId: string): Promise<JobSummary | null>;
    runTiming(repo: RepoRef, runId: string): Promise<RunTiming>;
    workflowFile(repo: RepoRef, workflowId: string): Promise<WorkflowFile>;
    artifacts(repo: RepoRef, runId: string): Promise<Artifact[]>;
    pendingApprovals(repo: RepoRef, runId: string): Promise<PendingApproval[]>;
    dispatch(repo: RepoRef, workflowId: string, gitRef: string, inputs: Record<string, string>): Promise<void>;
    rerun(repo: RepoRef, runId: string, failedOnly: boolean, debug?: boolean): Promise<void>;
    rerunJob(repo: RepoRef, jobId: string, debug?: boolean): Promise<void>;
    cancel(repo: RepoRef, runId: string): Promise<void>;
    deleteRunLogs(repo: RepoRef, runId: string): Promise<void>;
    deleteRun(repo: RepoRef, runId: string): Promise<void>;
    reviewDeployment(repo: RepoRef, runId: string, environmentIds: number[], state: "approved" | "rejected", comment?: string): Promise<void>;
    downloadArtifact(repo: RepoRef, artifactId: string, name: string, onProgress?: (progress: DownloadProgress) => void): Promise<SavedArtifact>;
    watchStart(repo: RepoRef, runId: string, onTick: (tick: RunTick) => void): Promise<number>;
    watchStop(streamId: number): Promise<void>;

    pulls(repo: RepoRef, state: string): Promise<Pull[]>;
    pull(repo: RepoRef, number: number): Promise<Pull>;
    pullFiles(repo: RepoRef, number: number): Promise<ChangedFile[]>;
    pullCommits(repo: RepoRef, number: number): Promise<PullCommit[]>;
    /** The accounts behind recent commits' emails on a branch, so local history can show who wrote it. */
    commitAuthors?(repo: RepoRef, gitRef: string | null): Promise<CommitAuthor[]>;
    pullReviews(repo: RepoRef, number: number): Promise<Review[]>;
    /** `of` says which a number names, for hosts that number pull requests and issues apart. */
    timeline(repo: RepoRef, number: number, of?: ThreadOf): Promise<TimelineItem[]>;
    /** `sha` is the head commit the person saw; the host refuses the merge if the branch has moved since. */
    mergePull(repo: RepoRef, number: number, method: MergeMethod, sha: string): Promise<void>;
    createPull(repo: RepoRef, pull: NewPull): Promise<Pull>;
    setPullState(repo: RepoRef, number: number, state: "open" | "closed"): Promise<void>;
    reviewPull(repo: RepoRef, number: number, event: ReviewEvent, body: string): Promise<void>;

    issues(repo: RepoRef, state: string, page: number): Promise<IssuePage>;
    issue(repo: RepoRef, number: number): Promise<Issue>;
    createIssue(repo: RepoRef, title: string, body: string): Promise<Issue>;
    setIssueState(repo: RepoRef, number: number, state: "open" | "closed"): Promise<void>;
    comments(repo: RepoRef, number: number, of?: ThreadOf): Promise<Comment[]>;
    addComment(repo: RepoRef, number: number, body: string, of?: ThreadOf): Promise<void>;

    releases(repo: RepoRef): Promise<Release[]>;
    downloadAsset(repo: RepoRef, assetId: number, name: string, onProgress?: (progress: DownloadProgress) => void): Promise<SavedArtifact>;

    inbox(account: string | null, all: boolean): Promise<Notification[]>;
    markRead(account: string | null, id: string): Promise<void>;
    markAllRead(account: string | null): Promise<void>;
}

export interface CodeHost {
    /** The plugin's id, which is also what `RepoRef.provider` holds. */
    readonly id: string;
    readonly name: string;
    /** What the host calls its CI, such as Actions or Pipelines. */
    readonly ciName: string;
    readonly icon: (size: number) => ReactNode;
    readonly capabilities: HostCapabilities;
    readonly api: CodeHostApi;
    /** A picture read straight from a commit email, such as GitHub's own noreply addresses, without asking the host. */
    readonly avatarForEmail?: (email: string) => string | null;
    /** An account's picture from its name alone, for places the host named someone without sending their picture. */
    readonly avatarForLogin?: (login: string) => string | null;
    /** The ref a pull request from a fork can be fetched by, such as GitHub's `pull/N/head`. */
    readonly pullHeadRef?: (number: number) => string;
    /** Shown in the Git pane while nobody is signed in to this host, and to add another account. */
    readonly SignIn: ComponentType<{ onSignedIn: (account: string | null) => void }>;
}

const hosts = new Map<string, CodeHost>();

export function registerCodeHost(host: CodeHost): void {
    if (hosts.has(host.id)) throw new Error(`code host ${host.id} is registered twice`);
    hosts.set(host.id, host);
}

export function codeHost(id: string): CodeHost | undefined {
    return hosts.get(id);
}

export function codeHosts(): readonly CodeHost[] {
    return [...hosts.values()];
}

/** Hosts whose plugin the person has not switched off, in the order they registered. */
export function enabledCodeHosts(): readonly CodeHost[] {
    return codeHosts().filter((host) => isPluginEnabled(host.id));
}

export function hostApi(id: string): CodeHostApi {
    const host = hosts.get(id);
    if (!host) throw new Error(`no code host ${id} is registered`);
    return host.api;
}

const HostContext = createContext<CodeHost | null>(null);
const AccountContext = createContext<string | null>(null);

/** The account the Git pane is showing its host as, or null for the host's default account. */
export const AccountProvider = AccountContext.Provider;

export function useAccount(): string | null {
    return useContext(AccountContext);
}

export const HostProvider = HostContext.Provider;

/** Someone's picture: the one the host sent, else whatever the host can make from their name. */
export function usePictureOf(login: string | null, url: string | null): string | null {
    const host = useContext(HostContext);
    return url ?? (login ? (host?.avatarForLogin?.(login) ?? null) : null);
}

/** The host the Git pane is showing, for the few reads that are about an account rather than a repository. */
export function useHost(): CodeHost {
    const host = useContext(HostContext);
    if (!host) throw new Error("useHost is only for what the Git pane draws inside a host");
    return host;
}
