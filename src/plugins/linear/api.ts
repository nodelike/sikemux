import { browserSignIn, createPluginBackend } from "../../plugin-api/backend";
import { LINEAR_PLUGIN } from "./kinds";

const backend = createPluginBackend(LINEAR_PLUGIN.id);

export interface LinearWorkspace {
    id: string;
    name: string;
    urlKey: string;
    user: string;
    isDefault: boolean;
}

export interface LinearStatus {
    configured: boolean;
    ok: boolean;
    authFailed: boolean;
    message: string | null;
    workspaces: LinearWorkspace[];
}

export interface LinearState {
    id: string;
    name: string;
    /** backlog, unstarted, started, completed, canceled or triage. */
    type: string;
    color: string;
    position?: number;
}

export interface LinearPerson {
    id: string;
    name: string;
    displayName: string;
}

export interface LinearLabel {
    id: string;
    name: string;
    color: string;
}

export interface LinearIssue {
    id: string;
    identifier: string;
    title: string;
    url: string;
    priority: number;
    priorityLabel: string;
    updatedAt: string;
    state: LinearState;
    assignee: LinearPerson | null;
    team: { id: string; key: string; name: string };
    labels: LinearLabel[];
}

export interface LinearComment {
    id: string;
    body: string;
    createdAt: string;
    user: { name: string; displayName: string } | null;
}

export interface LinearIssueDetail extends Omit<LinearIssue, "team"> {
    description: string | null;
    createdAt: string;
    branchName: string;
    creator: { name: string } | null;
    team: { id: string; key: string; name: string; states: LinearState[] };
    project: { name: string } | null;
    cycle: { number: number; name: string | null } | null;
    comments: LinearComment[];
}

export interface LinearChange {
    state?: string;
    assignee?: string;
    priority?: number;
}

export const linearApi = {
    status: () => backend.call<LinearStatus>("status"),
    signIn: (key: string) => backend.call<LinearStatus>("signIn", { key }),
    signInWithBrowser: (openPage: (url: string) => void) => browserSignIn<LinearStatus>(backend, "signInWithBrowser", openPage),
    signOut: (workspace: string) => backend.call<void>("signOut", { workspace }),
    search: (workspace: string, query: string) => backend.call<LinearIssue[]>("search", { workspace, query, limit: 50 }),
    issue: (workspace: string, id: string) => backend.call<LinearIssueDetail>("issue", { workspace, id }),
    comment: (workspace: string, id: string, body: string) => backend.call<{ id: string; url: string }>("comment", { workspace, id, body }),
    update: (workspace: string, id: string, change: LinearChange) => backend.call<LinearIssueDetail>("update", { workspace, id, ...change }),
};
