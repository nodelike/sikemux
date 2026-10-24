import type { JiraIssueSummary } from "./api";

/** The issues in any of the picked statuses, or all of them when none is picked. */
export function withStatuses(issues: JiraIssueSummary[], picked: ReadonlySet<string>): JiraIssueSummary[] {
    return picked.size === 0 ? issues : issues.filter((issue) => picked.has(issue.status));
}
