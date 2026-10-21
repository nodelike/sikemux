import { defineService } from "../../plugin-api/service";
import { jiraApi } from "./api";
import { JIRA_PLUGIN } from "./kinds";

export const jira = defineService(JIRA_PLUGIN, jiraApi.status);

export const jiraSearchR = jira.resource("search", (jql: string, site: string) => jiraApi.search(jql, site || undefined), 30_000);

export const jiraIssueR = jira.resource("issue", (key: string, site: string) => jiraApi.issue(key, site || undefined), 30_000);

export const jiraFiltersR = jira.resource("filters", (site: string) => jiraApi.filters(site || undefined), 300_000);

export const jiraProjectsR = jira.resource("projects", (site: string) => jiraApi.projects(site || undefined), 600_000);

export const jiraBoardsR = jira.resource("boards", (site: string) => jiraApi.boards(site || undefined), 600_000);

export const jiraBoardR = jira.resource(
    "board",
    (id: number, site: string, picked: string) => jiraApi.board(id, site || undefined, picked ? picked.split(",") : []),
    30_000,
);
