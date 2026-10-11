import { createPluginBackend, isPluginFailure } from "../../plugin-api/backend";
import { invalidate } from "../../plugin-api/resources";
import { JIRA_PLUGIN_ID } from "./kinds";

const backend = createPluginBackend(JIRA_PLUGIN_ID);

export interface JiraSite {
    host: string;
    displayName: string | null;
    default: boolean;
}

export interface JiraStatus {
    configured: boolean;
    sites: JiraSite[];
    ok: boolean;
    authFailed: boolean;
    message: string | null;
    browserSignIn: boolean;
}

export interface JiraPerson {
    accountId: string;
    name: string;
}

export type StatusCategory = "new" | "indeterminate" | "done" | "";

export interface JiraIssueSummary {
    key: string;
    summary: string;
    status: string;
    statusCategory: StatusCategory;
    priority: string | null;
    assignee: JiraPerson | null;
    issueType: string | null;
    sprint: string | null;
    updated: string | null;
    url: string;
}

export interface JiraComment {
    id: string;
    author: string;
    created: string;
    /** Markdown. */
    body: string;
}

export interface JiraTransition {
    id: string;
    name: string;
    to: string;
}

export interface JiraIssue extends JiraIssueSummary {
    project: string | null;
    reporter: JiraPerson | null;
    labels: string[];
    created: string | null;
    /** Markdown. */
    description: string;
    comments: JiraComment[];
    commentCount: number;
    transitions: JiraTransition[];
}

export interface JiraPage {
    issues: JiraIssueSummary[];
    next: string | null;
}

export interface JiraFilter {
    id: string;
    name: string;
    jql: string;
}

export function failureMessage(error: unknown): string {
    return isPluginFailure(error) ? error.message : String(error);
}

export function isSignedOut(error: unknown): boolean {
    if (!isPluginFailure(error)) return false;
    return (
        error.category === "auth" ||
        error.category === "unconfigured" ||
        (error.category === "http" && (error.status === 401 || error.status === 403))
    );
}

export const refreshJira = () => invalidate((kind) => kind.startsWith("jira."));

/** Only the sign-in status is checked again; refreshing every list would refetch the one that just failed, forever. */
async function read<T>(method: string, params?: unknown): Promise<T> {
    try {
        return await backend.call<T>(method, params);
    } catch (error) {
        if (isSignedOut(error)) invalidate((kind) => kind === "jira.status");
        throw error;
    }
}

interface BrowserSignInItem extends Partial<JiraStatus> {
    url?: string;
}

export interface BrowserSignIn {
    /** Resolves once the browser comes back and the account's sites are signed in, and rejects if it never does. */
    done: Promise<JiraStatus>;
    cancel(): void;
}

/** Waits for the browser to come back from Atlassian's sign-in page, which `openPage` is handed to show. */
function signInWithBrowser(openPage: (url: string) => void): BrowserSignIn {
    let cancel = () => {};
    const done = new Promise<JiraStatus>((resolve, reject) => {
        let status: JiraStatus | null = null;
        const stream = backend.stream<BrowserSignInItem>(
            "signInWithBrowser",
            {},
            {
                onItem: (item) => {
                    if (item.url) openPage(item.url);
                    else status = item as JiraStatus;
                },
                onEnd: () => (status ? resolve(status) : reject(new Error("the sign-in ended without a site"))),
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

export interface JiraProject {
    key: string;
    name: string;
}

export interface JiraBoard {
    id: number;
    name: string;
    kind: "scrum" | "kanban" | string;
    project: string | null;
}

export interface JiraSprint {
    id: number;
    name: string;
    end: string | null;
    goal: string | null;
}

export interface JiraColumn {
    name: string;
    statusIds: string[];
    issues: JiraIssueSummary[];
}

export interface JiraBoardView {
    id: number;
    name: string;
    kind: string;
    sprint: JiraSprint | null;
    columns: JiraColumn[];
    truncated: boolean;
}

export const jiraApi = {
    status: () => backend.call<JiraStatus>("status"),
    signIn: (site: string, email: string, token: string) => backend.call<JiraStatus>("signIn", { site, email, token }),
    signInWithBrowser,
    signOut: (site: string) => backend.call<void>("signOut", { site }),
    search: (jql: string, site?: string, next?: string) => read<JiraPage>("search", { jql, site, next, limit: 50 }),
    issue: (key: string, site?: string) => read<JiraIssue>("issue", { key, site }),
    comment: (key: string, body: string, site?: string) => read<JiraComment>("comment", { key, body, site }),
    transition: (key: string, to: string, site?: string) =>
        read<{ status: string | null; transitions: JiraTransition[] }>("transition", { key, to, site }),
    assign: (key: string, who: { accountId?: string | null; me?: boolean }, site?: string) => read<void>("assign", { key, ...who, site }),
    /** Ticks or clears the `index`th task of the description; `text` must still be its words. */
    setTask: (key: string, index: number, text: string, done: boolean, site?: string) => read<void>("setTask", { key, index, text, done, site }),
    assignable: (key: string, query: string, site?: string) => read<JiraPerson[]>("assignable", { key, query, site }),
    filters: (site?: string) => read<JiraFilter[]>("filters", { site }),
    projects: (site?: string) => read<JiraProject[]>("projects", { site }),
    boards: (site?: string) => read<JiraBoard[]>("boards", { site }),
    board: (id: number, site?: string) => read<JiraBoardView>("board", { id, site }),
    /** Moves an issue into a board column through whichever transition leads to one of its statuses. */
    moveIssue: (key: string, column: JiraColumn, site?: string) =>
        read<void>("moveIssue", { key, statusIds: column.statusIds, column: column.name, site }),
    keys: (text: string) => backend.call<string[]>("keys", { text }),
};
