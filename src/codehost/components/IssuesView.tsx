import type { ReactNode } from "react";
import { notify, reportError } from "../../plugin-api/host";
import { invalidate, useResourceEnabled } from "../../plugin-api/resources";
import { EmptyState, IconInfo, IconPlus, SkeletonRows, Tooltip } from "../../plugin-api/ui";
import { GitColumns } from "../../git/GitColumns";
import { hostApi, failureMessage, type Issue, type RepoRef } from "../api";
import { issueR, issuesR } from "../resources";
import { formatAgo } from "../runStatus";
import { compose, setListState, showItem, updateView } from "../state";
import { Comments, Labels, PageHead, StateMark, stateLabel, Who } from "./Bits";
import { CommentThread } from "./CommentThread";
import { useBusy, useNow } from "./hooks";
import { NewIssueForm } from "./NewIssueForm";
import { IconAgent } from "../../ui/Icons";
import { workOnIssue } from "../workOnIssue";

const LIST_STATES = ["open", "closed", "all"];

/** Starts a chat in the project with this issue in its input. Only offered where the issue's repository is the project's own. */
function WorkOnThis({ repo, number, cwd, compact = false }: { repo: RepoRef; number: number; cwd: string; compact?: boolean }) {
    const [busy, runBusy] = useBusy();
    const start = () => runBusy(() => workOnIssue(repo, number, cwd));
    if (compact)
        return (
            <Tooltip label="Work on this">
                <button type="button" className="gha-icon-btn gha-work-on" aria-label={`Work on #${number}`} disabled={busy} onClick={start}>
                    <IconAgent size={13} />
                </button>
            </Tooltip>
        );
    return (
        <button type="button" className="gha-btn" disabled={busy} onClick={start}>
            <IconAgent size={12} /> Work on this
        </button>
    );
}

function IssueRow({ issue, now, selected, onOpen, work }: { issue: Issue; now: number; selected: boolean; onOpen: () => void; work: ReactNode }) {
    return (
        <div className="gha-item-line">
            <button type="button" className="gha-item-row" data-on={selected ? "1" : "0"} onClick={onOpen}>
                <StateMark kind="issue" state={issue.state} reason={issue.stateReason} />
                <span className="gha-item-head">
                    <span className="gha-item-title">{issue.title}</span>
                    <Labels labels={issue.labels} />
                </span>
                <Comments count={issue.comments} />
                <span className="gha-item-sub">
                    <span className="gha-item-number">#{issue.number}</span>
                    {issue.author && <Who login={issue.author} avatarUrl={issue.avatarUrl} />}
                    {issue.assignees.length > 0 && <span>→ {issue.assignees.join(", ")}</span>}
                </span>
                <span className="gha-item-when">{formatAgo(issue.updatedAt, now)}</span>
            </button>
            {work}
        </div>
    );
}

export function IssueDetail({ repo, number, active, cwd = null }: { repo: RepoRef; number: number; active: boolean; cwd?: string | null }) {
    const issue = useResourceEnabled(active, issueR, repo, number);
    const now = useNow(false);
    const [busy, runBusy] = useBusy();
    if (issue.status === "loading" && !issue.data) return <SkeletonRows rows={6} label="Loading issue" />;
    if (!issue.data) {
        return <EmptyState title="Could not read it" message={failureMessage(issue.error)} tone="error" />;
    }
    const found = issue.data;
    const closing = found.state === "open";

    const setState = () =>
        runBusy(() =>
            hostApi(repo.provider)
                .setIssueState(repo, found.number, closing ? "closed" : "open")
                .then(() => {
                    notify("success", closing ? `Closed #${found.number}` : `Reopened #${found.number}`);
                    invalidate((kind) => kind === "host.issue" || kind === "host.issues");
                })
                .catch(reportError(closing ? "Could not close it" : "Could not reopen it")),
        );

    return (
        <div className="gha-detail">
            <PageHead
                mark={<StateMark kind="issue" state={found.state} reason={found.stateReason} size={14} />}
                title={found.title}
                number={found.number}
                url={found.url}
                actions={cwd && <WorkOnThis repo={repo} number={found.number} cwd={cwd} />}>
                <span
                    className="gha-state-word"
                    data-kind="issue"
                    data-state={found.state === "closed" && found.stateReason === "not_planned" ? "not_planned" : found.state}>
                    {stateLabel("issue", found.state, false, found.stateReason)}
                </span>
                <Who login={found.author} avatarUrl={found.avatarUrl} />
                <span>opened {formatAgo(found.createdAt, now)}</span>
                {found.assignees.length > 0 && <span>→ {found.assignees.join(", ")}</span>}
                <Labels labels={found.labels} />
            </PageHead>
            <CommentThread
                repo={repo}
                number={found.number}
                of="issue"
                active={active}
                now={now}
                opening={{
                    key: "opening",
                    author: found.author,
                    avatarUrl: found.avatarUrl,
                    association: null,
                    at: found.createdAt,
                    body: found.body,
                    review: null,
                }}
                extraActions={
                    <button type="button" className="gha-btn" disabled={busy} onClick={setState}>
                        {closing ? "Close issue" : "Reopen issue"}
                    </button>
                }
            />
        </div>
    );
}

interface Props {
    paneId: string;
    repo: RepoRef;
    listState: string;
    item: number | null;
    composing: boolean;
    page: number;
    /** The project folder, when the issues are its own repository's, so an agent can be started on one there. */
    cwd: string | null;
    active: boolean;
}

export function IssuesView({ paneId, repo, listState, item, composing, page, cwd, active }: Props) {
    const issues = useResourceEnabled(active && !composing, issuesR, repo, listState, page);
    const now = useNow(false);

    if (composing) return <NewIssueForm repo={repo} onCreated={(number) => showItem(paneId, number)} onCancel={() => compose(paneId, null)} />;

    const rows = issues.data?.issues ?? [];
    const total = issues.data?.total ?? rows.length;
    const nextPage = issues.data?.nextPage ?? null;
    const list =
        issues.status === "loading" && !issues.data ? (
            <SkeletonRows rows={8} label="Loading issues" />
        ) : issues.error ? (
            <EmptyState
                title="Could not read issues"
                message={failureMessage(issues.error)}
                tone="error"
                action={{ label: "Try again", onClick: () => void issues.refresh() }}
            />
        ) : (
            <div className="gha-list pr-list">
                <div className="gha-list-head">
                    <div className="gha-chips">
                        {LIST_STATES.map((state) => (
                            <button
                                key={state}
                                type="button"
                                className="gha-chip"
                                data-on={listState === state ? "1" : "0"}
                                onClick={() => setListState(paneId, "issues", state)}>
                                {state === "all" ? "All" : state === "open" ? "Open" : "Closed"}
                            </button>
                        ))}
                    </div>
                    <span className="gha-page-spacer" />
                    <span className="gha-dim">{total}</span>
                    <Tooltip label="New issue">
                        <button type="button" className="gha-icon-btn" aria-label="New issue" onClick={() => compose(paneId, "issue")}>
                            <IconPlus size={13} />
                        </button>
                    </Tooltip>
                </div>
                {rows.length === 0 ? (
                    <EmptyState icon={<IconInfo size={20} />} message={`No ${listState === "all" ? "" : listState} issues.`} />
                ) : (
                    rows.map((issue) => (
                        <IssueRow
                            key={issue.number}
                            issue={issue}
                            now={now}
                            selected={issue.number === item}
                            onOpen={() => showItem(paneId, issue.number)}
                            work={cwd && issue.state === "open" && <WorkOnThis repo={repo} number={issue.number} cwd={cwd} compact />}
                        />
                    ))
                )}
                {(page > 1 || nextPage) && (
                    <div className="gha-pager">
                        <button type="button" className="gha-btn" disabled={page <= 1} onClick={() => updateView(paneId, { page: page - 1 })}>
                            Newer
                        </button>
                        <span className="gha-dim">Page {page}</span>
                        <button type="button" className="gha-btn" disabled={!nextPage} onClick={() => updateView(paneId, { page: page + 1 })}>
                            Older
                        </button>
                    </div>
                )}
            </div>
        );

    const right =
        item === null ? (
            <EmptyState icon={<IconInfo size={20} />} message="Pick an issue to read it." />
        ) : (
            <div className="issue-page">
                <IssueDetail repo={repo} number={item} active={active} cwd={cwd} />
            </div>
        );
    return <GitColumns paneId={paneId} left={list} right={right} />;
}
