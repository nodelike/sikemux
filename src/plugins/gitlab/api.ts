import {
    createCodeHostBackend,
    namedArgsHostApi,
    type CodeHostApi,
    type HostAccount,
    type HostAccountEntry,
    type HostCapabilities,
} from "../../plugin-api/codehost";
import { GITLAB_PLUGIN } from "./kinds";

const client = createCodeHostBackend(GITLAB_PLUGIN.id);
const { backend } = client;

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
export const gitlabCapabilities: HostCapabilities = {
    ci: {
        graph: true,
        attempts: false,
        approvals: false,
        dispatch: true,
        annotations: false,
        summaries: false,
        artifacts: false,
        billing: false,
        workflowFile: true,
        rerunFailed: true,
        rerunJob: true,
        debugLogs: false,
        deleteRuns: true,
        deleteLogs: false,
    },
    pulls: { draft: true, mergeMethods: ["merge", "squash"], requestChanges: false, reopen: true, mergeability: true },
    issues: true,
    releases: true,
    inbox: true,
};

/** GitLab as the Git pane reads any code host. */
export const gitlabHostApi: CodeHostApi = {
    ...namedArgsHostApi(GITLAB_PLUGIN.name, client),
    rerunJob: (repo, jobId) => client.call<void>("rerunJob", { ...repo, jobId }),
    deleteRun: (repo, runId) => client.call<void>("deleteRun", { ...repo, runId }),
    status: (account) => gitlabApi.status(account).then(accountOf),
    accounts: () => backend.call<ListedAccount[]>("accounts").then((listed) => listed.map(entryOf)),
};
