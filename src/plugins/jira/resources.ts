import { resource } from "../../plugin-api/resources";
import {
    jiraApi,
    type JiraBoard,
    type JiraBoardView,
    type JiraFilter,
    type JiraIssue,
    type JiraPage,
    type JiraProject,
    type JiraStatus,
} from "./api";

export const jiraStatusR = resource({
    kind: "jira.status",
    fetch: (): Promise<JiraStatus> => jiraApi.status(),
    staleAfterMs: 60_000,
});

export const jiraSearchR = resource({
    kind: "jira.search",
    fetch: (jql: string, site: string): Promise<JiraPage> => jiraApi.search(jql, site || undefined),
    staleAfterMs: 30_000,
});

export const jiraIssueR = resource({
    kind: "jira.issue",
    fetch: (key: string, site: string): Promise<JiraIssue> => jiraApi.issue(key, site || undefined),
    staleAfterMs: 30_000,
});

export const jiraFiltersR = resource({
    kind: "jira.filters",
    fetch: (site: string): Promise<JiraFilter[]> => jiraApi.filters(site || undefined),
    staleAfterMs: 300_000,
});

export const jiraKeysR = resource({
    kind: "jira.keys",
    fetch: (text: string): Promise<string[]> => jiraApi.keys(text),
    staleAfterMs: 3_600_000,
});

export const jiraProjectsR = resource({
    kind: "jira.projects",
    fetch: (site: string): Promise<JiraProject[]> => jiraApi.projects(site || undefined),
    staleAfterMs: 600_000,
});

export const jiraBoardsR = resource({
    kind: "jira.boards",
    fetch: (site: string): Promise<JiraBoard[]> => jiraApi.boards(site || undefined),
    staleAfterMs: 600_000,
});

export const jiraBoardR = resource({
    kind: "jira.board",
    fetch: (id: number, site: string): Promise<JiraBoardView> => jiraApi.board(id, site || undefined),
    staleAfterMs: 30_000,
});
