import type { JiraIssueSummary, StatusCategory } from "./api";

export interface StatusCount {
    status: string;
    category: StatusCategory;
    count: number;
}

const CATEGORY_ORDER: StatusCategory[] = ["indeterminate", "new", "", "done"];

/** The statuses in a list with how many issues sit in each, work in progress first and finished work last. */
export function statusCounts(issues: JiraIssueSummary[]): StatusCount[] {
    const counts = new Map<string, StatusCount>();
    for (const issue of issues) {
        const seen = counts.get(issue.status);
        if (seen) seen.count += 1;
        else counts.set(issue.status, { status: issue.status, category: issue.statusCategory, count: 1 });
    }
    return [...counts.values()].sort(
        (a, b) => CATEGORY_ORDER.indexOf(a.category) - CATEGORY_ORDER.indexOf(b.category) || a.status.localeCompare(b.status),
    );
}

/** The issues in any of the picked statuses, or all of them when none is picked. */
export function withStatuses(issues: JiraIssueSummary[], picked: ReadonlySet<string>): JiraIssueSummary[] {
    return picked.size === 0 ? issues : issues.filter((issue) => picked.has(issue.status));
}
