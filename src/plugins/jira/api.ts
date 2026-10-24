import { browserSignIn, createPluginBackend } from "../../plugin-api/backend";
import { JIRA_PLUGIN } from "./kinds";
import type { JiraColumn, JiraComment, JiraPerson, JiraTransition } from "./types.generated";

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

export type {
    JiraActivity,
    JiraBoard,
    JiraBoardPage,
    JiraBoardPerson,
    JiraBoardView,
    JiraChange,
    JiraColumn,
    JiraComment,
    JiraFilter,
    JiraIssueList,
    JiraIssueListKind,
    JiraIssueSummary,
    JiraIssueView,
    JiraParent,
    JiraPerson,
    JiraProject,
    JiraSpace,
    JiraSpaces,
    JiraSprint,
    JiraStatusCount,
    JiraSwimlane,
    JiraTransition,
} from "./types.generated";

const avatars = new Map<string, Promise<string>>();

export const jiraApi = {
    status: () => backend.call<JiraStatus>("status"),
    signIn: (site: string, email: string, token: string) => backend.call<JiraStatus>("signIn", { site, email, token }),
    /** Waits for the browser to come back from Atlassian's sign-in page with the account's sites signed in. */
    signInWithBrowser: (openPage: (url: string) => void) => browserSignIn<JiraStatus>(backend, "signInWithBrowser", openPage),
    signOut: (site: string) => backend.call<void>("signOut", { site }),
    comment: (key: string, body: string, site?: string) => backend.call<JiraComment>("comment", { key, body, site }),
    transition: (key: string, to: string, site?: string) =>
        backend.call<{ status: string | null; transitions: JiraTransition[] }>("transition", { key, to, site }),
    assign: (key: string, who: { accountId?: string | null; me?: boolean }, site?: string) => backend.call<void>("assign", { key, ...who, site }),
    /** Ticks or clears the `index`th task of the description; `text` must still be its words. */
    setTask: (key: string, index: number, text: string, done: boolean, site?: string) =>
        backend.call<void>("setTask", { key, index, text, done, site }),
    assignable: (key: string, query: string, site?: string) => backend.call<JiraPerson[]>("assignable", { key, query, site }),
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
