import { Segmented } from "../../ui/Controls";
import { useState, type ReactNode } from "react";
import { notify, openUrl, reportError, swallow } from "../../plugin-api/host";
import { invalidate, useResourceEnabled } from "../../plugin-api/resources";
import { EmptyState, IconChevron, IconExternal, IconInfo, IconPlus, SkeletonRows, StaleNotice, Tooltip } from "../../plugin-api/ui";
import { SwipePages } from "../../ui/SwipePages";
import { SectionIcon } from "./ActionsIcon";
import { GitColumns } from "../../git/GitColumns";
import { hostApi, type Issue, type RepoRef } from "../api";
import { errorText } from "../../lib/format";
import { issueR, timelineR } from "../resources";
import { issueListR } from "../lists";
import { compose, setListState, showItem, updateView } from "../state";
import { Comments, Labels, ListTitle, ShellAccount, StateMark } from "./Bits";
import { CommentThread, Face } from "./CommentThread";
import { useHost } from "../registry";
import { useBusy } from "./hooks";
import { useNow } from "../../plugin-api/hooks";
import { NewIssueForm } from "./NewIssueForm";
import { IconAgent } from "../../ui/Icons";
import { workOnIssue } from "../workOnIssue";
import { formatAgo } from "../../lib/format";

const LIST_STATES = ["open", "closed", "all"];

/** Starts a chat in the project with this issue in its input. Only offered where the issue's repository is the project's own. */
function WorkOnThis({ number, cwd, compact = false }: { number: number; cwd: string; compact?: boolean }) {
    const [busy, runBusy] = useBusy();
    const start = () => runBusy(() => workOnIssue(number, cwd));
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

/** An issue as the full list shows it, read like the pull request list: who opened it, its state and title, then its number, who and when, with who it is assigned to, its comments and starting work at the end. */
function IssueListRow({ issue, now, onOpen, work }: { issue: Issue; now: number; onOpen: () => void; work: ReactNode }) {
    const opened = formatAgo(issue.createdAt, now);
    const updated = formatAgo(issue.updatedAt, now);
    return (
        <div className="issue-row">
            <button type="button" className="pr-row pr-headline" onClick={onOpen}>
                <span className="pr-face">
                    <Face login={issue.author} url={issue.avatarUrl} />
                </span>
                <span className="pr-headline-top">
                    <StateMark kind="issue" state={issue.state} reason={issue.stateReason} size={14} />
                    <span className="pr-headline-title">{issue.title}</span>
                </span>
                <span className="pr-headline-sub">
                    <span className="pr-headline-who">
                        #{issue.number} · {issue.author ?? "someone"} opened {opened}
                        {updated !== opened && <> · updated {updated}</>}
                    </span>
                    <Labels labels={issue.labels} />
                </span>
                <span className="pr-row-side">
                    {issue.assignees.length > 0 && (
                        <span className="pr-row-reviewers" title={`Assigned to ${issue.assignees.join(", ")}`}>
                            {issue.assignees.map((login) => (
                                <Face key={login} login={login} url={null} />
                            ))}
                        </span>
                    )}
                    <Comments count={issue.comments} />
                </span>
            </button>
            {work}
        </div>
    );
}

/** An issue as the side panel lists it beside the open one: its mark and title, then its number, who and when. */
function IssueRow({ issue, now, selected, onOpen }: { issue: Issue; now: number; selected: boolean; onOpen: () => void }) {
    return (
        <button
            type="button"
            className="side-row"
            data-on={selected ? "1" : "0"}
            ref={(node) => {
                if (node && selected) node.scrollIntoView({ block: "nearest" });
            }}
            onClick={onOpen}>
            <StateMark kind="issue" state={issue.state} reason={issue.stateReason} size={13} />
            <span className="side-row-title">{issue.title}</span>
            <Comments count={issue.comments} />
            <span className="side-row-sub">
                #{issue.number} · {issue.author ?? "someone"} · {formatAgo(issue.updatedAt, now)}
            </span>
        </button>
    );
}

/** What GitHub keeps in an issue's sidebar, each fact a row with its name beside it: who it is on, its labels, who took part and the pull requests that mention it. */
function IssueFacts({ repo, number, active }: { repo: RepoRef; number: number; active: boolean }) {
    const issue = useResourceEnabled(active, issueR, repo, number).data;
    const timeline = useResourceEnabled(active, timelineR, repo, number, "issue").data ?? [];
    if (!issue) return <SkeletonRows rows={4} label="Loading issue" />;
    const people = [...new Set([issue.author, ...timeline.map((item) => item.actor)].filter((login): login is string => !!login))];
    const linked = [
        ...new Map(
            timeline.filter((item) => item.kind === "cross-referenced" && item.source?.pull).map((item) => [item.source?.number, item] as const),
        ).values(),
    ];
    return (
        <div className="issue-facts">
            <div className="issue-fact">
                <span className="issue-fact-name">Assignees</span>
                <span className="issue-fact-value">
                    {issue.assignees.length === 0 ? (
                        <span className="gha-dim">No one</span>
                    ) : (
                        issue.assignees.map((login) => (
                            <span key={login} className="issue-fact-person">
                                <Face login={login} url={null} />
                                {login}
                            </span>
                        ))
                    )}
                </span>
            </div>
            <div className="issue-fact">
                <span className="issue-fact-name">Labels</span>
                <span className="issue-fact-value">
                    {issue.labels.length === 0 ? <span className="gha-dim">None</span> : <Labels labels={issue.labels} />}
                </span>
            </div>
            <div className="issue-fact">
                <span className="issue-fact-name">Participants</span>
                <span className="issue-fact-value issue-fact-faces" title={people.join(", ")}>
                    {people.map((login) => (
                        <Face key={login} login={login} url={null} />
                    ))}
                </span>
            </div>
            <div className="issue-fact">
                <span className="issue-fact-name">Comments</span>
                <span className="issue-fact-value">{issue.comments}</span>
            </div>
            <div className="issue-fact issue-fact-block">
                <span className="issue-fact-name">Linked pull requests</span>
                {linked.length === 0 ? (
                    <span className="gha-dim">None</span>
                ) : (
                    linked.map((item) => (
                        <span key={item.source?.number} className="issue-fact-link">
                            <StateMark kind="pull" state={item.source?.merged ? "merged" : (item.source?.state ?? "open")} size={13} />
                            <span className="issue-fact-link-title">{item.subject}</span>
                            <span className="gha-dim">#{item.source?.number}</span>
                        </span>
                    ))
                )}
            </div>
        </div>
    );
}

export function IssueDetail({ repo, number, active, cwd = null }: { repo: RepoRef; number: number; active: boolean; cwd?: string | null }) {
    const host = useHost();
    const issue = useResourceEnabled(active, issueR, repo, number);
    const now = useNow(false);
    const [busy, runBusy] = useBusy();
    if (issue.status === "loading" && !issue.data) return <SkeletonRows rows={6} label="Loading issue" />;
    if (!issue.data) {
        return <EmptyState title="Could not read it" message={errorText(issue.error)} tone="error" />;
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
        <div className="pr-right">
            <div className="git-detail">
                <div className="pr-headline">
                    <span className="pr-face">
                        <Face login={found.author} url={found.avatarUrl} />
                    </span>
                    <span className="pr-headline-top">
                        <StateMark kind="issue" state={found.state} reason={found.stateReason} size={14} />
                        <h2 className="git-detail-title pr-headline-title">{found.title}</h2>
                        <span className="issue-head-actions">
                            {cwd && <WorkOnThis number={found.number} cwd={cwd} />}
                            <Tooltip label={`Open on ${host.name}`}>
                                <button
                                    type="button"
                                    className="gha-icon-btn"
                                    aria-label={`Open on ${host.name}`}
                                    onClick={() => void openUrl(found.url).catch(swallow(`open ${host.name}`))}>
                                    <IconExternal size={12} />
                                </button>
                            </Tooltip>
                        </span>
                    </span>
                    <span className="pr-headline-sub">
                        <span className="pr-headline-who">
                            #{found.number} · {found.author ?? "someone"} opened {formatAgo(found.createdAt, now)}
                            {found.assignees.length > 0 && <> · assigned to {found.assignees.join(", ")}</>}
                        </span>
                        <Labels labels={found.labels} />
                    </span>
                </div>
            </div>
            <div className="pr-conversation">
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
        </div>
    );
}

interface Props {
    paneId: string;
    repo: RepoRef;
    project: string;
    listState: string;
    item: number | null;
    composing: boolean;
    page: number;
    /** The project folder, when the issues are its own repository's, so an agent can be started on one there. */
    cwd: string | null;
    active: boolean;
}

export function IssuesView({ paneId, repo, project, listState, item, composing, page, cwd, active }: Props) {
    const issues = useResourceEnabled(active && !composing, issueListR, project, listState, page);
    const [sidePage, setSidePage] = useState("facts");
    const counts = issues.data?.counts;
    const now = useNow(false);

    if (composing) return <NewIssueForm repo={repo} onCreated={(number) => showItem(paneId, number)} onCancel={() => compose(paneId, null)} />;

    const rows = issues.data?.issues ?? [];
    const total = issues.data?.total ?? rows.length;
    const nextPage = issues.data?.nextPage ?? null;
    const list =
        issues.status === "loading" && !issues.data ? (
            <SkeletonRows rows={8} label="Loading issues" />
        ) : issues.error && !issues.data ? (
            <EmptyState
                title="Could not read issues"
                message={errorText(issues.error)}
                tone="error"
                action={{ label: "Try again", onClick: () => void issues.refresh() }}
            />
        ) : (
            <div className="gha-list pr-list">
                <StaleNotice handle={issues} />
                <div className="gha-list-head">
                    <ListTitle section="issues" />
                    <Segmented
                        label="Which to list"
                        value={listState}
                        options={LIST_STATES.map((state) => ({
                            value: state,
                            label: state === "all" ? "All" : state === "open" ? "Open" : "Closed",
                            count: counts && state !== "all" ? counts[state as "open" | "closed"] : undefined,
                        }))}
                        onChange={(state) => setListState(paneId, "issues", state)}
                    />
                    <span className="gha-page-spacer" />
                    <span className="gha-dim">{total}</span>
                    <Tooltip label="New issue">
                        <button type="button" className="gha-icon-btn" aria-label="New issue" onClick={() => compose(paneId, "issue")}>
                            <IconPlus size={13} />
                        </button>
                    </Tooltip>
                    <ShellAccount />
                </div>
                {rows.length === 0 ? (
                    <EmptyState icon={<IconInfo size={20} />} message={`No ${listState === "all" ? "" : listState} issues.`} />
                ) : (
                    <div className="pr-rows">
                        {rows.map((issue) => (
                            <IssueListRow
                                key={issue.number}
                                issue={issue}
                                now={now}
                                onOpen={() => showItem(paneId, issue.number)}
                                work={cwd && issue.state === "open" && <WorkOnThis number={issue.number} cwd={cwd} compact />}
                            />
                        ))}
                    </div>
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

    if (item === null) return <div className="gha-full-page">{list}</div>;
    const openIssue = rows.find((issue) => issue.number === item);
    const side = (
        <div className="git-side">
            <div className="git-side-head">
                <div className="git-side-title">
                    <Tooltip label="Back to the issues">
                        <button
                            type="button"
                            className="git-foot-btn icon pr-back-btn"
                            aria-label="Back to the issues"
                            onClick={() => showItem(paneId, null)}>
                            <IconChevron size={11} />
                        </button>
                    </Tooltip>
                    <StateMark kind="issue" state={openIssue?.state ?? "open"} reason={openIssue?.stateReason ?? null} size={14} />
                    <span className="pr-side-number">#{item}</span>
                    <span className="gha-page-spacer" />
                    <Segmented
                        small
                        label="Which to list"
                        value={listState}
                        options={LIST_STATES.map((state) => ({
                            value: state,
                            label: state === "all" ? "All" : state === "open" ? "Open" : "Closed",
                        }))}
                        onChange={(state) => setListState(paneId, "issues", state)}
                    />
                    <Tooltip label="New issue">
                        <button type="button" className="git-foot-btn icon" aria-label="New issue" onClick={() => compose(paneId, "issue")}>
                            <IconPlus size={13} />
                        </button>
                    </Tooltip>
                    <ShellAccount />
                </div>
            </div>
            <SwipePages
                label="Issue"
                current={sidePage}
                onChange={setSidePage}
                pages={[
                    {
                        id: "facts",
                        label: "Details",
                        icon: <IconInfo size={14} />,
                        content: <IssueFacts repo={repo} number={item} active={active} />,
                    },
                    {
                        id: "list",
                        label: "Issues",
                        icon: <SectionIcon section="issues" size={14} />,
                        content: (
                            <div className="git-list side-list">
                                {rows.map((issue) => (
                                    <IssueRow
                                        key={issue.number}
                                        issue={issue}
                                        now={now}
                                        selected={issue.number === item}
                                        onOpen={() => showItem(paneId, issue.number)}
                                    />
                                ))}
                            </div>
                        ),
                    },
                ]}
            />
        </div>
    );
    return (
        <GitColumns
            paneId={paneId}
            left={side}
            right={
                <div className="git-right-review">
                    <IssueDetail repo={repo} number={item} active={active} cwd={cwd} />
                </div>
            }
        />
    );
}
