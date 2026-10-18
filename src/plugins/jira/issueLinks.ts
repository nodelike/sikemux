import { createElement } from "react";
import { copyWithNotice, openUrl, swallow } from "../../plugin-api/host";
import { IconCopy, IconExternal, type ContextMenuItem } from "../../plugin-api/ui";

export interface LinkedIssue {
    key: string;
    summary: string;
    url: string;
}

export function openIssue(issue: LinkedIssue): void {
    void openUrl(issue.url).catch(swallow("open Jira"));
}

export function copyIssue(text: string, what: string): void {
    copyWithNotice(text, `the ${what}`);
}

/** Ways to take an issue out of Sikemux: to the browser, or to the clipboard in the shapes people paste. */
export function issueMenu(issue: LinkedIssue): ContextMenuItem[] {
    const copy = createElement(IconCopy, { size: 13 });
    return [
        { label: "Open in browser", icon: createElement(IconExternal, { size: 13 }), run: () => openIssue(issue) },
        { sep: true },
        { label: "Copy link", icon: copy, run: () => copyIssue(issue.url, "link") },
        { label: "Copy key", icon: copy, run: () => copyIssue(issue.key, "key") },
        { label: "Copy key and title", icon: copy, run: () => copyIssue(`${issue.key} ${issue.summary}`, "key and title") },
        { label: "Copy as Markdown link", icon: copy, run: () => copyIssue(`[${issue.key}](${issue.url}) ${issue.summary}`, "Markdown link") },
    ];
}
