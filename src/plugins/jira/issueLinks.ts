import { copyText, notify, openUrl, reportError, swallow } from "../../plugin-api/host";
import type { ContextMenuItem } from "../../plugin-api/ui";

export interface LinkedIssue {
    key: string;
    summary: string;
    url: string;
}

export function openIssue(issue: LinkedIssue): void {
    void openUrl(issue.url).catch(swallow("open Jira"));
}

export function copyIssue(text: string, what: string): void {
    void copyText(text).then(() => notify("success", `copied the ${what}`), reportError("copy"));
}

/** Ways to take an issue out of Sikemux: to the browser, or to the clipboard in the shapes people paste. */
export function issueMenu(issue: LinkedIssue): ContextMenuItem[] {
    return [
        { label: "Open in browser", run: () => openIssue(issue) },
        { sep: true },
        { label: "Copy link", run: () => copyIssue(issue.url, "link") },
        { label: "Copy key", run: () => copyIssue(issue.key, "key") },
        { label: "Copy key and title", run: () => copyIssue(`${issue.key} ${issue.summary}`, "key and title") },
        { label: "Copy as Markdown link", run: () => copyIssue(`[${issue.key}](${issue.url}) ${issue.summary}`, "Markdown link") },
    ];
}
