import { create } from "zustand";
import { activeSurfacePane, onPaneClosed, openSurface } from "../../plugin-api/host";
import { JIRA_ISSUES } from "./kinds";

export type JiraList =
    | { kind: "mine" }
    | { kind: "reported" }
    | { kind: "watching" }
    | { kind: "recent" }
    | { kind: "sprint" }
    | { kind: "project"; key: string; name: string }
    | { kind: "unassigned"; key: string; name: string }
    | { kind: "board"; id: number; name: string }
    | { kind: "filter"; id: string; name: string; jql: string }
    | { kind: "jql"; jql: string };

/** Everything still open that is mine, and what I finished in the last two weeks so a done ticket does not just vanish. */
export const MINE_JQL = "assignee = currentUser() AND (statusCategory != Done OR updated >= -14d) ORDER BY updated DESC";
export const SPRINT_JQL = "sprint in openSprints() ORDER BY Rank ASC";
export const REPORTED_JQL = "reporter = currentUser() AND (statusCategory != Done OR updated >= -14d) ORDER BY updated DESC";
export const WATCHING_JQL = "watcher = currentUser() AND statusCategory != Done ORDER BY updated DESC";
export const RECENT_JQL = "issuekey in issueHistory() ORDER BY lastViewed DESC";

/** A project key as JQL can hold it, in quotes in case the key is also a JQL word. */
const projectClause = (key: string) => `project = "${key.replace(/[^A-Za-z0-9_]/g, "")}"`;

const JQL_OPERATOR = /[=~<>]|\border\s+by\b|\bin\s*\(|\bis\s+(not\s+)?(empty|null)\b/i;

/** What is typed in the search box: JQL when it reads as JQL, otherwise words to find in any issue's text. */
export function searchJql(typed: string): string {
    const text = typed.trim();
    if (JQL_OPERATOR.test(text)) return text;
    const quoted = text.replace(/\\/g, "\\\\").replace(/"/g, '\\"');
    return `text ~ "${quoted}" ORDER BY updated DESC`;
}

/** The JQL behind a list; a board has none, since it is read as the board itself. */
export function jqlOf(list: Exclude<JiraList, { kind: "board" }>): string {
    switch (list.kind) {
        case "mine":
            return MINE_JQL;
        case "reported":
            return REPORTED_JQL;
        case "watching":
            return WATCHING_JQL;
        case "recent":
            return RECENT_JQL;
        case "sprint":
            return SPRINT_JQL;
        case "project":
            return `${projectClause(list.key)} AND statusCategory != Done ORDER BY updated DESC`;
        case "unassigned":
            return `${projectClause(list.key)} AND assignee is EMPTY AND statusCategory != Done ORDER BY created DESC`;
        case "jql":
            return searchJql(list.jql);
        case "filter":
            return list.jql;
    }
}

/** Whether a commit message names the issue: as a whole word, in any case, so ABC-12 is not ABC-123. */
export function mentions(text: string, key: string): boolean {
    const escaped = key.replace(/[^A-Za-z0-9-]/g, "");
    return escaped.length > 0 && new RegExp(`(^|[^A-Za-z0-9])${escaped}(?![0-9])`, "i").test(text);
}

export interface JiraView {
    list: JiraList;
    /** The issue open beside the list. */
    issue: string | null;
    /** The site shown; empty for the default one. */
    site: string;
}

const FIRST_VIEW: JiraView = { list: { kind: "mine" }, issue: null, site: "" };

const useViews = create<{ views: Record<string, JiraView> }>(() => ({ views: {} }));

onPaneClosed((paneId) =>
    useViews.setState((state) => {
        const views = { ...state.views };
        delete views[paneId];
        return { views };
    }),
);

export const useJiraView = (paneId: string): JiraView => useViews((state) => state.views[paneId] ?? FIRST_VIEW);

export function updateJiraView(paneId: string, change: Partial<JiraView>): void {
    useViews.setState((state) => ({ views: { ...state.views, [paneId]: { ...(state.views[paneId] ?? FIRST_VIEW), ...change } } }));
}

export const openJira = (): string | null => openSurface(JIRA_ISSUES);

/** Shows an issue in the Jira pane, opening the pane if none is. */
export function openJiraIssue(key: string): void {
    const paneId = activeSurfacePane(JIRA_ISSUES) ?? openSurface(JIRA_ISSUES);
    if (paneId) updateJiraView(paneId, { issue: key });
}
