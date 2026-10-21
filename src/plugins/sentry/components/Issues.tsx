import { useState } from "react";
import { Seg } from "../../../plugin-api/ui";
import { errorText } from "../../../plugin-api/format";
import { sentryApi, type Issue, type IssuePeriod, type IssueQuery, type IssueSort, type IssueStatusFilter } from "../api";
import { ago, count, levelTone } from "../format";
import { Avatar, LevelMark, Sparkline, Substatus } from "./parts";

const STATUSES: { value: IssueStatusFilter; label: string }[] = [
    { value: "unresolved", label: "Unresolved" },
    { value: "assigned", label: "Assigned to me" },
    { value: "resolved", label: "Resolved" },
    { value: "ignored", label: "Ignored" },
    { value: "all", label: "All" },
];

const SORTS: { value: IssueSort; label: string }[] = [
    { value: "date", label: "Last seen" },
    { value: "new", label: "First seen" },
    { value: "freq", label: "Events" },
    { value: "user", label: "Users" },
];

const PERIODS: { value: IssuePeriod; label: string }[] = [
    { value: "24h", label: "24h" },
    { value: "14d", label: "14d" },
];

export function Issues({
    issues,
    next,
    query,
    showProject,
    onQuery,
    onOpen,
}: {
    issues: Issue[];
    next: string | null;
    query: IssueQuery;
    showProject: boolean;
    onQuery: (change: Partial<Pick<IssueQuery, "status" | "sort" | "period">>) => void;
    onOpen: (issue: Issue) => void;
}) {
    const [more, setMore] = useState<{ issues: Issue[]; next: string | null; for: string } | null>(null);
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const key = JSON.stringify(query);
    const extra = more?.for === key ? more : null;
    const shown = extra ? [...issues, ...extra.issues.filter((each) => !issues.some((first) => first.id === each.id))] : issues;
    const cursor = extra ? extra.next : next;

    const loadMore = async () => {
        if (!cursor) return;
        setLoading(true);
        setError(null);
        try {
            const page = await sentryApi.issues(query, cursor);
            setMore({ issues: [...(extra?.issues ?? []), ...page.issues], next: page.next, for: key });
        } catch (failure) {
            setError(errorText(failure));
        } finally {
            setLoading(false);
        }
    };

    return (
        <section className="sentry-section" aria-label="Issues">
            <div className="sentry-toolbar">
                <Seg label="Status" value={query.status} onChange={(status) => onQuery({ status })} options={STATUSES} />
                <span className="sentry-toolbar-gap" />
                <Seg label="Sort" value={query.sort} onChange={(sort) => onQuery({ sort })} options={SORTS} />
                <Seg label="Period" value={query.period} onChange={(period) => onQuery({ period })} options={PERIODS} />
            </div>
            {shown.length === 0 ? (
                <p className="sentry-note">{query.status === "unresolved" ? "No unresolved issues. Nothing is on fire." : "No issues match."}</p>
            ) : (
                <ul className="sentry-issues" aria-label="Issues">
                    <li className="sentry-issues-head" aria-hidden="true">
                        <span>Issue</span>
                        <span>{query.period === "24h" ? "Last 24h" : "Last 14 days"}</span>
                        <span className="sentry-num">Events</span>
                        <span className="sentry-num">Users</span>
                        <span>Seen</span>
                        <span />
                    </li>
                    {shown.map((issue) => (
                        <li key={issue.id}>
                            <button
                                type="button"
                                className="sentry-issue"
                                aria-label={`${issue.shortId} ${issue.title}`}
                                onClick={() => onOpen(issue)}>
                                <LevelMark tone={levelTone(issue.level)} label={issue.level} />
                                <span className="sentry-issue-main">
                                    <span className="sentry-issue-title">
                                        {issue.kind ? <strong>{issue.kind}</strong> : null}
                                        <span>{issue.kind && issue.message ? issue.message : issue.title}</span>
                                    </span>
                                    <span className="sentry-issue-meta">
                                        <span className="sentry-mono">{issue.shortId}</span>
                                        {showProject ? <span className="sentry-chip">{issue.project}</span> : null}
                                        <Substatus value={issue.substatus} />
                                        {issue.unhandled ? <span className="sentry-chip unhandled">Unhandled</span> : null}
                                        <span className="sentry-culprit">{issue.culprit}</span>
                                    </span>
                                </span>
                                <Sparkline values={issue.trend} />
                                <span className="sentry-num">{count(issue.events)}</span>
                                <span className="sentry-num">{count(issue.users)}</span>
                                <span className="sentry-seen">
                                    <span>{ago(issue.lastSeen)} ago</span>
                                    <span className="sentry-muted">{ago(issue.firstSeen)} old</span>
                                </span>
                                <span className="sentry-assignee">{issue.assignee ? <Avatar name={issue.assignee.name} /> : null}</span>
                            </button>
                        </li>
                    ))}
                </ul>
            )}
            {error ? <p className="sentry-note sentry-error">{error}</p> : null}
            {cursor ? (
                <button type="button" className="sentry-button sentry-more" disabled={loading} onClick={() => void loadMore()}>
                    {loading ? "Loading…" : "Load more"}
                </button>
            ) : null}
        </section>
    );
}
