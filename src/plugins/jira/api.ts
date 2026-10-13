import { browserSignIn, createPluginBackend } from "../../plugin-api/backend";
import { invalidate } from "../../plugin-api/resources";
import { JIRA_PLUGIN_ID } from "./kinds";

export { failureMessage } from "../../plugin-api/backend";

const backend = createPluginBackend(JIRA_PLUGIN_ID, { signInStatus: (kind) => kind === "jira.status" });

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

export const refreshJira = () => invalidate((kind) => kind.startsWith("jira."));

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
    /** Waits for the browser to come back from Atlassian's sign-in page with the account's sites signed in. */
    signInWithBrowser: (openPage: (url: string) => void) => browserSignIn<JiraStatus>(backend, "signInWithBrowser", openPage),
    signOut: (site: string) => backend.call<void>("signOut", { site }),
    search: (jql: string, site?: string, next?: string) => backend.call<JiraPage>("search", { jql, site, next, limit: 50 }),
    issue: (key: string, site?: string) => backend.call<JiraIssue>("issue", { key, site }),
    comment: (key: string, body: string, site?: string) => backend.call<JiraComment>("comment", { key, body, site }),
    transition: (key: string, to: string, site?: string) =>
        backend.call<{ status: string | null; transitions: JiraTransition[] }>("transition", { key, to, site }),
    assign: (key: string, who: { accountId?: string | null; me?: boolean }, site?: string) => backend.call<void>("assign", { key, ...who, site }),
    /** Ticks or clears the `index`th task of the description; `text` must still be its words. */
    setTask: (key: string, index: number, text: string, done: boolean, site?: string) =>
        backend.call<void>("setTask", { key, index, text, done, site }),
    assignable: (key: string, query: string, site?: string) => backend.call<JiraPerson[]>("assignable", { key, query, site }),
    filters: (site?: string) => backend.call<JiraFilter[]>("filters", { site }),
    projects: (site?: string) => backend.call<JiraProject[]>("projects", { site }),
    boards: (site?: string) => backend.call<JiraBoard[]>("boards", { site }),
    board: (id: number, site?: string) => backend.call<JiraBoardView>("board", { id, site }),
    /** Moves an issue into a board column through whichever transition leads to one of its statuses. */
    moveIssue: (key: string, column: JiraColumn, site?: string) =>
        backend.call<void>("moveIssue", { key, statusIds: column.statusIds, column: column.name, site }),
    keys: (text: string) => backend.call<string[]>("keys", { text }),
};
