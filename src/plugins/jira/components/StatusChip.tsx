import "../jiraChip.css";

/** A status, coloured by the bucket Jira puts it in: to do, in progress or done. */
export function StatusChip({ status, category }: { status: string; category: string }) {
    return <span className={`jira-status jira-status-${category || "new"}`}>{status}</span>;
}
