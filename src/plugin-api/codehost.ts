import { createCodeHostClient, type CodeHostClient } from "../codehost/client";
import { hostStatusR } from "../codehost/resources";
import type { CodeHostApi } from "../codehost/registry";
import type {
    ChangedFile,
    Comment,
    Issue,
    IssuePage,
    Notification,
    Pull,
    PullCommit,
    Release,
    RepoListing,
    Resolved,
    Review,
    RunPage,
    TimelineItem,
    Workflow,
    WorkflowFile,
    RateLimit,
} from "../codehost/types";
import { createPluginBackend, type PluginBackend } from "./backend";

export type * from "../codehost/types";
export {
    registerCodeHost,
    type CodeHost,
    type CodeHostApi,
    type HostAccount,
    type HostAccountEntry,
    type HostCapabilities,
    type ThreadOf,
} from "../codehost/registry";
export type { CodeHostClient } from "../codehost/client";
export { hostCiGlyph } from "../codehost/components/HostCiGlyph";

/** A code host plugin's backend. A call that finds the person signed out has the Git pane's read of the host's sign-in read again too. */
export function createCodeHostBackend(pluginId: string): CodeHostClient & { readonly backend: PluginBackend } {
    const backend = createPluginBackend(pluginId, { alsoStatus: [hostStatusR.kind] });
    return { backend, ...createCodeHostClient(backend) };
}

export type CodeHostBackend = ReturnType<typeof createCodeHostBackend>;

/**
 * The Git pane's calls for a host whose plugin names every id as text and takes every argument by name, as GitLab's
 * and Bitbucket's do. What the host has no counterpart for reads as empty, or is refused if it is a change; a host
 * whose capabilities say it can re-run a job or delete runs or their logs adds that call itself.
 */
export function namedArgsHostApi(name: string, host: CodeHostBackend): Omit<CodeHostApi, "status" | "accounts"> {
    const { backend, call, image } = host;
    const cannot = (what: string) => (): Promise<never> => Promise.reject(new Error(`${name} cannot ${what}`));
    return {
        rateLimit: (account) => backend.call<RateLimit>("rateLimit", { account }),
        signOut: (account) => backend.call<void>("signOut", { account }),
        setDefaultAccount: (account) => backend.call<void>("setDefaultAccount", { id: account }),
        accountFor: (repo) => call<string | null>("accountFor", repo),
        resolveRemote: (url) => backend.call<Resolved>("resolveRemote", { url }),
        myRepos: (account, limit = 50) => call<RepoListing[]>("myRepos", { account, limit }),
        image,

        workflows: (repo) => call<Workflow[]>("workflows", repo),
        branches: (repo) => call<string[]>("branches", repo),
        runs: (query) => call<RunPage>("runs", query),
        annotations: () => Promise.resolve([]),
        jobSummary: () => Promise.resolve(null),
        workflowFile: (repo) => call<WorkflowFile>("workflowFile", repo),
        artifacts: () => Promise.resolve([]),
        pendingApprovals: () => Promise.resolve([]),
        dispatch: (repo, workflowId, gitRef, inputs) => call<void>("dispatch", { ...repo, workflowId, gitRef, inputs }),
        rerun: (repo, runId, failedOnly) => call<void>("rerun", { ...repo, runId, failedOnly }),
        rerunJob: cannot("re-run a single job"),
        cancel: (repo, runId) => call<void>("cancel", { ...repo, runId }),
        deleteRunLogs: cannot("delete a run's logs alone"),
        deleteRun: cannot("delete a run"),
        reviewDeployment: cannot("approve a deployment"),
        downloadArtifact: cannot("download artifacts"),

        pulls: (repo, state) => call<Pull[]>("pulls", { ...repo, state }),
        pull: (repo, number) => call<Pull>("pull", { ...repo, number }),
        pullFiles: (repo, number) => call<ChangedFile[]>("pullFiles", { ...repo, number, fullPatches: true }),
        pullCommits: (repo, number) => call<PullCommit[]>("pullCommits", { ...repo, number }),
        pullReviews: (repo, number) => call<Review[]>("pullReviews", { ...repo, number }),
        timeline: (repo, number, of = "pull") => call<TimelineItem[]>("timeline", { ...repo, number, of }),
        mergePull: (repo, number, method, sha) => call<void>("mergePull", { ...repo, number, method, sha }),
        createPull: (repo, pull) => call<Pull>("createPull", { ...repo, ...pull }),
        setPullState: (repo, number, state) => call<void>("setPullState", { ...repo, number, state }),
        reviewPull: (repo, number, event, body) => call<void>("reviewPull", { ...repo, number, event, body }),

        issues: (repo, state, page) => call<IssuePage>("issues", { ...repo, state, page }),
        issue: (repo, number) => call<Issue>("issue", { ...repo, number }),
        createIssue: (repo, title, body) => call<Issue>("createIssue", { ...repo, title, body }),
        setIssueState: (repo, number, state) => call<void>("setIssueState", { ...repo, number, state }),
        comments: (repo, number, of = "pull") => call<Comment[]>("comments", { ...repo, number, of }),
        addComment: (repo, number, body, of = "pull") => call<void>("addComment", { ...repo, number, body, of }),

        releases: (repo) => call<Release[]>("releases", repo),
        downloadAsset: cannot("download release files"),

        inbox: (account, all) => call<Notification[]>("inbox", { account, all }),
        markRead: (account, id) => call<void>("markRead", { account, id }),
        markAllRead: (account) => call<void>("markAllRead", { account }),
    };
}
