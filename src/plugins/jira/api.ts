import { browserSignIn, createPluginBackend } from "../../plugin-api/backend";
import { JIRA_PLUGIN } from "./kinds";

const backend = createPluginBackend(JIRA_PLUGIN.id);

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
    avatarUrl: string | null;
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
    /** The day it is due, as `2026-10-31`. */
    due: string | null;
    /** The epic or other issue it sits under. */
    parent: JiraParent | null;
    url: string;
}

export interface JiraParent {
    key: string;
    summary: string;
    /** Jira's name for the epic's colour: `color_1` to `color_14`, or a name such as `teal`. */
    color: string | null;
}

export interface JiraComment {
    id: string;
    author: string;
    authorAvatarUrl: string | null;
    created: string;
    /** Markdown. */
    body: string;
}

/** One edit to an issue and the fields it changed. */
export interface JiraChange {
    id: string;
    author: string;
    authorAvatarUrl: string | null;
    created: string;
    fields: { field: string; from: string | null; to: string | null }[];
}

export interface JiraTransition {
    id: string;
    name: string;
    to: string;
    toCategory: StatusCategory;
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
    /** The latest edits, newest first. */
    history: JiraChange[];
    transitions: JiraTransition[];
    subtasks: JiraIssueSummary[];
    /** The keys of the issues this one is blocked by. */
    blockedBy: string[];
    /** The keys of the issues this one blocks. */
    blocks: string[];
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

export interface JiraProject {
    key: string;
    name: string;
    avatarUrl: string | null;
}

export interface JiraBoard {
    id: number;
    name: string;
    kind: "scrum" | "kanban" | string;
    project: string | null;
    /** The project's key when the board belongs to one rather than to a person. */
    projectKey: string | null;
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

const avatars = new Map<string, Promise<string>>();

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
    /** A board, narrowed to the cards of the people picked: account ids, plus "me" and "none" for unassigned. */
    board: (id: number, site?: string, picked: string[] = []) =>
        backend.call<JiraBoardView>("board", {
            id,
            site,
            people: picked.filter((each) => each !== "me" && each !== "none"),
            mine: picked.includes("me"),
            unassigned: picked.includes("none"),
        }),
    /** Moves an issue into a board column through whichever transition leads to one of its statuses. */
    moveIssue: (key: string, column: JiraColumn, site?: string) =>
        backend.call<void>("moveIssue", { key, statusIds: column.statusIds, column: column.name, site }),
    /** A person's picture as a `data:` address, since the window cannot load Atlassian's images itself. */
    avatar(url: string): Promise<string> {
        const known = avatars.get(url);
        if (known) return known;
        const fetched = backend.call<string>("image", { url });
        avatars.set(url, fetched);
        fetched.catch(() => avatars.delete(url));
        return fetched;
    },
    /** A project's picture as a `data:` address; Jira serves these only to someone signed in. */
    siteImage(url: string, site?: string): Promise<string> {
        const known = avatars.get(url);
        if (known) return known;
        const fetched = backend.call<string>("siteImage", { url, site });
        avatars.set(url, fetched);
        fetched.catch(() => avatars.delete(url));
        return fetched;
    },
};
