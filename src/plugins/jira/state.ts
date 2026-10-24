import { openSurface } from "../../plugin-api/host";
import { definePaneViews } from "../../plugin-api/paneViews";
import { definePluginSettings } from "../../plugin-api/settings";
import type { JiraIssueListKind } from "./api";
import { JIRA_ISSUES, JIRA_PLUGIN } from "./kinds";

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

/** What the core lists for a pane's list; a board has none, since it is read as the board itself. */
export function listOf(list: Exclude<JiraList, { kind: "board" }>): JiraIssueListKind {
    switch (list.kind) {
        case "project":
        case "unassigned":
            return { kind: list.kind, key: list.key };
        case "filter":
            return { kind: "filter", jql: list.jql };
        case "jql":
            return { kind: "search", text: list.jql };
        default:
            return { kind: list.kind };
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

const views = definePaneViews<JiraView>({ list: { kind: "mine" }, issue: null, site: "" });

export const useJiraView = views.use;
export const updateJiraView = views.update;

export interface JiraSettings {
    /** How wide the open issue is, in pixels; null for the width that follows the pane. */
    detailWidth: number | null;
}

export const jiraSettings = definePluginSettings(JIRA_PLUGIN.id, (saved): JiraSettings => {
    const width = (saved as { detailWidth?: unknown } | null)?.detailWidth;
    return { detailWidth: typeof width === "number" ? width : null };
});

export const openJira = (): string | null => openSurface(JIRA_ISSUES);
