import {
    createCodeHostBackend,
    namedArgsHostApi,
    type Annotation,
    type Artifact,
    type CodeHostApi,
    type CommitAuthor,
    type DownloadProgress,
    type HostAccount,
    type HostAccountEntry,
    type HostCapabilities,
    type JobSummary,
    type PendingApproval,
    type RunDetail,
    type SavedArtifact,
    type WorkflowFile,
} from "../../plugin-api/codehost";
import { GITHUB_PLUGIN } from "./kinds";

export type * from "../../plugin-api/codehost";
const client = createCodeHostBackend(GITHUB_PLUGIN.id);
const { backend, call } = client;

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
            onError: reject,
        });
    });
}

export const actionsApi = {
    status: (account: string | null = null) => backend.call<ActionsStatus>("status", { account }),
    signIn: (host: string, token?: string) => backend.call<ActionsStatus>("signIn", { host, token }),
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

export const githubCapabilities: HostCapabilities = {
    ci: {
        graph: true,
        attempts: true,
        approvals: true,
        dispatch: true,
        annotations: true,
        summaries: true,
        artifacts: true,
        billing: true,
        workflowFile: true,
        rerunFailed: true,
        rerunJob: true,
        debugLogs: true,
        deleteRuns: true,
        deleteLogs: true,
    },
    pulls: { draft: true, mergeMethods: ["squash", "merge", "rebase"], requestChanges: true, reopen: true, mergeability: true },
    issues: true,
    releases: true,
    inbox: true,
};

/** GitHub as the Git pane reads any code host: the shared calls, and everything Actions has that other hosts' CI does not. */
export const githubHostApi: CodeHostApi = {
    ...namedArgsHostApi(GITHUB_PLUGIN.name, client),
    status: (account) => actionsApi.status(account).then(accountOf),
    accounts: () => backend.call<ListedAccount[]>("accounts").then((listed) => listed.map(entryOf)),

    workflowFile: (repo, workflowId) => call<WorkflowFile>("workflowFile", { ...repo, workflowId }),
    workflowFileAt: (repo, path, gitRef) => call<WorkflowFile>("workflowFileAt", { ...repo, path, gitRef }),
    runAttempt: (repo, runId, attempt) => call<RunDetail>("runAttempt", { ...repo, runId, attempt }),
    annotations: (repo, checkRunId) => call<Annotation[]>("annotations", { ...repo, checkRunId }),
    jobSummary: (repo, checkRunId) => call<JobSummary | null>("jobSummary", { ...repo, checkRunId }),
    artifacts: (repo, runId) => call<Artifact[]>("artifacts", { ...repo, runId }),
    pendingApprovals: (repo, runId) => call<PendingApproval[]>("pendingApprovals", { ...repo, runId }),
    rerun: (repo, runId, failedOnly, debug = false) => call<void>("rerun", { ...repo, runId, failedOnly, debug }),
    rerunJob: (repo, jobId, debug = false) => call<void>("rerunJob", { ...repo, jobId, debug }),
    deleteRunLogs: (repo, runId) => call<void>("deleteRunLogs", { ...repo, runId }),
    deleteRun: (repo, runId) => call<void>("deleteRun", { ...repo, runId }),
    reviewDeployment: (repo, runId, environmentIds, state, comment = "") =>
        call<void>("reviewDeployment", { ...repo, runId, environmentIds, state, comment }),
    downloadArtifact: (repo, artifactId, name, onProgress) => download("downloadArtifact", { ...repo, artifactId, fileName: name }, onProgress),
    downloadAsset: (repo, assetId, name, onProgress) => download("downloadAsset", { ...repo, assetId, fileName: name }, onProgress),

    pullCounts: (repo) => call<{ open: number; closed: number }>("pullCounts", { ...repo }),
    issueCounts: (repo) => call<{ open: number; closed: number }>("issueCounts", { ...repo }),
    commitAuthors: (repo, gitRef) => call<CommitAuthor[]>("commitAuthors", { ...repo, gitRef }),
};

const NOREPLY = /^(\d+)\+[^@]+@users\.noreply\.github\.com$/iu;

/** GitHub's private commit emails carry the account's number, which is all its avatar address needs. */
export function avatarForEmail(email: string): string | null {
    const found = NOREPLY.exec(email.trim());
    return found ? `https://avatars.githubusercontent.com/u/${found[1]}?s=64` : null;
}
