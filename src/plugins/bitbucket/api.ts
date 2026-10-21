import { browserSignIn } from "../../plugin-api/backend";
import {
    createCodeHostBackend,
    namedArgsHostApi,
    type CodeHostApi,
    type CommitAuthor,
    type HostAccount,
    type HostAccountEntry,
    type HostCapabilities,
} from "../../plugin-api/codehost";
import { BITBUCKET_PLUGIN } from "./kinds";

const client = createCodeHostBackend(BITBUCKET_PLUGIN.id);
const { backend } = client;

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

export const bitbucketCapabilities: HostCapabilities = {
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
        rerunFailed: false,
        rerunJob: false,
        debugLogs: false,
        deleteRuns: false,
        deleteLogs: false,
    },
    pulls: { draft: true, mergeMethods: ["squash", "merge"], requestChanges: true, reopen: false, mergeability: true },
    issues: true,
    releases: true,
    inbox: true,
};

/** Bitbucket as the Git pane reads any code host. */
export const bitbucketHostApi: CodeHostApi = {
    ...namedArgsHostApi(BITBUCKET_PLUGIN.name, client),
    status: (account) => bitbucketApi.status(account).then(accountOf),
    accounts: () => backend.call<ListedAccount[]>("accounts").then((listed) => listed.map(entryOf)),
    commitAuthors: (repo, gitRef) => client.call<CommitAuthor[]>("commitAuthors", { ...repo, gitRef }),
};
