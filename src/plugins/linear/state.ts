import { openSurface } from "../../plugin-api/host";
import { definePaneViews } from "../../plugin-api/paneViews";
import type { LinearIssueDetail } from "./api";
import { LINEAR_ISSUES } from "./kinds";

export interface LinearView {
    /** The workspace shown; empty for the default one. */
    workspace: string;
    /** Words searched for; empty lists the person's open issues. */
    query: string;
    /** The issue open beside the list, by identifier. */
    issue: string | null;
}

const views = definePaneViews<LinearView>({ workspace: "", query: "", issue: null });

export const useLinearView = views.use;
export const updateLinearView = views.update;

export const openLinear = (): string | null => openSurface(LINEAR_ISSUES);

/** An issue as an agent reads it: what it is, where it stands, what it asks for and what was said. */
export function issueForAgent(issue: LinearIssueDetail): string {
    const facts = [
        `State: ${issue.state.name}`,
        issue.assignee ? `Assignee: ${issue.assignee.displayName || issue.assignee.name}` : "Unassigned",
        issue.priorityLabel && issue.priority > 0 ? `Priority: ${issue.priorityLabel}` : null,
        issue.labels.length > 0 ? `Labels: ${issue.labels.map((label) => label.name).join(", ")}` : null,
        `Branch: ${issue.branchName}`,
    ].filter(Boolean);
    const comments = issue.comments.map((comment) => `**${comment.user?.displayName || comment.user?.name || "Someone"}**: ${comment.body}`);
    return [
        `Linear issue ${issue.identifier}: ${issue.title} — ${issue.url}`,
        facts.join("\n"),
        issue.description?.trim() || "(No description.)",
        comments.length > 0 ? `Comments:\n\n${comments.join("\n\n")}` : null,
        `To update it, use linear_update or linear_comment with id ${issue.identifier}.`,
    ]
        .filter(Boolean)
        .join("\n\n");
}
