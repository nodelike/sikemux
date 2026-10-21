import { createPluginBackend } from "../../plugin-api/backend";
import { SENTRY_PLUGIN } from "./kinds";

const backend = createPluginBackend(SENTRY_PLUGIN.id);

export interface SentryStatus {
    configured: boolean;
    ok: boolean;
    authFailed: boolean;
    message: string | null;
    url: string;
    /** The organisation's slug. */
    org: string;
    orgName: string;
    /** The project shown first, by slug; none for every project. */
    project: string | null;
    user: string;
}

export interface Organization {
    slug: string;
    name: string;
}

export interface Project {
    id: string;
    slug: string;
    name: string;
    platform: string | null;
}

export interface Member {
    /** What Sentry takes to assign an issue, such as user:42. */
    actor: string;
    name: string;
    email: string | null;
    isMe: boolean;
}

export interface Assignee {
    kind: "user" | "team" | string;
    id: string;
    name: string;
}

export type IssueLevel = "fatal" | "error" | "warning" | "info" | "debug" | string;

export interface Issue {
    id: string;
    shortId: string;
    title: string;
    culprit: string;
    kind: string | null;
    message: string | null;
    level: IssueLevel;
    status: "unresolved" | "resolved" | "ignored" | string;
    substatus: string | null;
    events: number;
    users: number;
    firstSeen: string | null;
    lastSeen: string | null;
    project: string;
    link: string;
    assignee: Assignee | null;
    unhandled: boolean;
    /** Events per hour or per day over the period, oldest first. */
    trend: number[];
}

export interface Frame {
    file: string | null;
    module: string | null;
    function: string | null;
    line: number | null;
    column: number | null;
    inApp: boolean;
    context: [number, string][];
}

export interface Exception {
    kind: string;
    value: string;
    handled: boolean | null;
    /** Oldest call first; the last one threw. */
    frames: Frame[];
}

export interface Breadcrumb {
    at: string | null;
    category: string;
    level: string;
    message: string;
}

export interface SentryEvent {
    id: string;
    at: string | null;
    message: string | null;
    /** The cause first, the one that reached the top last. */
    exceptions: Exception[];
    breadcrumbs: Breadcrumb[];
    tags: [string, string][];
    request: string | null;
    contexts: [string, string][];
    release: string | null;
    environment: string | null;
}

export interface IssueDetail extends Issue {
    firstRelease: string | null;
    lastRelease: string | null;
    comments: number;
    event: SentryEvent | null;
}

export interface Release {
    version: string;
    short: string;
    created: string | null;
    newIssues: number;
    commits: number;
    deployedTo: string | null;
    deployedAt: string | null;
    crashFreeSessions: number | null;
    crashFreeUsers: number | null;
    sessions: number;
}

export type IssueStatusFilter = "unresolved" | "assigned" | "resolved" | "ignored" | "all";
export type IssueSort = "date" | "new" | "freq" | "user";
export type IssuePeriod = "24h" | "14d";
export type IssueAction = "resolve" | "resolveInNextRelease" | "unresolve" | "ignore";

export interface IssueQuery {
    projects: string[];
    query: string;
    status: IssueStatusFilter;
    sort: IssueSort;
    environment: string | null;
    period: IssuePeriod;
}

export const sentryApi = {
    status: () => backend.call<SentryStatus>("status"),
    signIn: (token: string, url: string) => backend.call<SentryStatus>("signIn", { token, url }),
    signOut: () => backend.call<null>("signOut"),
    organizations: () => backend.call<Organization[]>("organizations"),
    choose: (choice: { org?: string; project?: string }) => backend.call<SentryStatus>("choose", choice),
    projects: () => backend.call<Project[]>("projects", {}),
    members: () => backend.call<Member[]>("members", {}),
    environments: (project: string | null) => backend.call<string[]>("environments", { project: project ?? undefined }),
    issues: (query: IssueQuery, cursor?: string) =>
        backend.call<{ issues: Issue[]; next: string | null }>("issues", {
            ...query,
            environment: query.environment ?? undefined,
            limit: 50,
            cursor,
        }),
    issue: (id: string) => backend.call<IssueDetail>("issue", { id }),
    /** The issue as the agent tool tells it, so an agent handed it from the pane reads the same. */
    brief: (id: string) => backend.call<string>("brief", { id }),
    change: (id: string, change: { action?: IssueAction; assign?: string }) => backend.call<Issue>("change", { id, ...change }),
    comment: (id: string, text: string) => backend.call<null>("comment", { id, text }),
    releases: (project: Project) => backend.call<Release[]>("releases", { project: project.slug, projectId: project.id }),
};
