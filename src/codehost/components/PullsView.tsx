import { Segmented } from "../../ui/Controls";
import { useMemo, useState, type ReactNode } from "react";
import { confirmDialog, copyText, notify, openUrl, reportError, swallow } from "../../plugin-api/host";
import { useHost } from "../registry";
import { useResourceEnabled } from "../../plugin-api/resources";
import {
    Dropdown,
    EmptyState,
    IconCheck,
    IconCheckCircle,
    IconCheckout,
    IconChevron,
    IconCommit,
    IconExternal,
    IconFile,
    IconMerge,
    IconPlus,
    IconPullRequest,
    SkeletonRows,
    StaleNotice,
    Tooltip,
} from "../../plugin-api/ui";
import type { GitCommit } from "../../api/git";
import { CommitReview } from "../../git/CommitReview";
import { GitColumns } from "../../git/GitColumns";
import { AuthorPicturesProvider, type AuthorPictures } from "../../git/AuthorAvatar";
import { GitGraph } from "../../git/GitGraph";
import { hostFileStatus } from "../../git/gitFileStatus";
import { FileIcon } from "../../ui/FileIcon";
import { SwipePages } from "../../ui/SwipePages";
import { basename, dirname } from "../../lib/paths";
import { requestOpenFile } from "../../state/commands";
import {
    hostApi,
    type CheckTally,
    type ListedPull,
    type MergeMethod,
    type MergeReadiness,
    type OpenPull,
    type Pull,
    type RepoRef,
    type ReviewVerdict,
} from "../api";
import { errorText } from "../../lib/format";
import { pullCommitsR, timelineR } from "../resources";
import { checkoutPull, pullFilesR, pullListR, pullViewR } from "../lists";
import { tallySummary, type Outcome } from "../runStatus";
import { needsPull } from "../compose";
import { compose, openRunFrom, setListState, showItem } from "../state";
import { CommentThread, Face } from "./CommentThread";
import { SectionIcon, StatusMark } from "./ActionsIcon";
import { pullState } from "../project";
import { Branch, Comments, Labels, ListTitle, ShellAccount, StateMark, Who } from "./Bits";
import { useBusy } from "./hooks";
import { useNow } from "../../plugin-api/hooks";
import { useCiWatch } from "../ciWatch";
import { NewPullForm } from "./NewPullForm";
import { PullFiles } from "./PullFiles";
import { PullChecks } from "./PullChecks";
import { formatAgo } from "../../lib/format";

const LIST_STATES = ["open", "closed", "all"];

/** Who opened it, its state and title, then its number, age and branches: one headline for the list and the open pull request. */
function PullHeadline({
    pull,
    now,
    title,
    trailing,
    meta,
    labels = false,
    state: listed,
}: {
    pull: Pull;
    now: number;
    title: ReactNode;
    trailing?: ReactNode;
    /** Sits at the end of the line under the title, after its branches. */
    meta?: ReactNode;
    /** The open pull request's header: when it last changed and its labels, rather than the list's opened and updated. */
    labels?: boolean;
    /** Its state as the core listed it, with a draft and a merge told apart. */
    state?: string;
}) {
    const state = listed ?? pullState(pull);
    return (
        <>
            <span className="pr-face">
                <Face login={pull.author} url={pull.avatarUrl} />
            </span>
            <span className="pr-headline-top">
                <StateMark kind="pull" state={state} size={14} />
                {title}
                {trailing}
            </span>
            <span className="pr-headline-sub">
                {labels ? (
                    <span className="pr-headline-who">
                        {pull.author ?? "someone"} · #{pull.number}
                        <span className="pr-headline-ago">, updated {formatAgo(pull.updatedAt, now)}</span>
                    </span>
                ) : (
                    <span className="pr-headline-who">
                        #{pull.number} · {pull.author ?? "someone"} opened {formatAgo(pull.createdAt, now)}
                        {formatAgo(pull.updatedAt, now) !== formatAgo(pull.createdAt, now) && <> · updated {formatAgo(pull.updatedAt, now)}</>}
                    </span>
                )}
                {pull.head && <span className="pr-ref pr-ref-head">{pull.head}</span>}
                {pull.head && pull.base && <span className="pr-ref-arrow">→</span>}
                {pull.base && <span className="pr-ref">{pull.base}</span>}
                {labels && <Labels labels={pull.labels} />}
                {meta}
            </span>
        </>
    );
}

function PullCi({ tally }: { tally: CheckTally }) {
    const summary = tallySummary(tally);
    return (
        <Tooltip label={summary === "all passed" ? "All checks passed" : `Checks: ${summary}`}>
            <span className="pr-ci" data-outcome={tally.outcome}>
                <StatusMark outcome={tally.outcome} size={13} />
                <span className="pr-ci-count">
                    {tally.passed}/{tally.total}
                </span>
            </span>
        </Tooltip>
    );
}

function PullRow({ listed, now, onOpen }: { listed: ListedPull; now: number; onOpen: () => void }) {
    const { pull, closes: issues } = listed;
    const ci = listed.checks ? <PullCi tally={listed.checks} /> : null;
    return (
        <button type="button" className="pr-row pr-headline" onClick={onOpen}>
            <PullHeadline
                pull={pull}
                state={listed.state}
                now={now}
                title={<span className="pr-headline-title">{pull.title}</span>}
                meta={ci && <span className="pr-ci-meta">{ci}</span>}
            />
            <span className="pr-row-side">
                {pull.reviewers.length > 0 && (
                    <span className="pr-row-reviewers" title={`Reviewers: ${pull.reviewers.join(", ")}`}>
                        {pull.reviewers.map((login) => (
                            <Face key={login} login={login} url={pull.avatars[login] ?? null} />
                        ))}
                    </span>
                )}
                {issues.length > 0 && (
                    <span className="pr-row-issues" title={`Closes ${issues.map((number) => `#${number}`).join(", ")}`}>
                        <SectionIcon section="issues" size={13} />
                        {issues.length}
                    </span>
                )}
                <Comments count={pull.comments ?? 0} />
            </span>
        </button>
    );
}

interface DetailProps {
    repo: RepoRef;
    project: string;
    cwd: string | null;
    projectBranch: string | null;
    number: number;
    active: boolean;
    login: string | null;
    onBack: () => void;
    onOpenRun: (runId: string) => void;
}

const MERGE_METHODS: { value: MergeMethod; label: string }[] = [
    { value: "squash", label: "Squash" },
    { value: "merge", label: "Merge commit" },
    { value: "rebase", label: "Rebase" },
];

function mergeability(merge: MergeReadiness, base: string, hostName: string): { outcome: Outcome; title: string; detail: string | null } {
    switch (merge) {
        case "conflicts":
            return { outcome: "failure", title: `Conflicts with ${base}`, detail: "Resolve the conflicts before merging." };
        case "blocked":
            return { outcome: "blocked", title: "Merging is blocked", detail: `${hostName} is waiting on the required reviews and checks.` };
        case "behind":
            return { outcome: "blocked", title: `Behind ${base}`, detail: "Bring the branch up to date before merging." };
        case "ready":
            return { outcome: "success", title: "Ready to merge", detail: null };
        case "checking":
            return { outcome: "queued", title: "Checking whether it can merge", detail: null };
    }
}

const VERDICT_LABEL: Record<ReviewVerdict, string> = { approved: "Approved", changesRequested: "Changes requested" };

function MergePart({ outcome, title, detail }: { outcome: Outcome; title: string; detail?: string | null }) {
    return (
        <div className="gha-merge-part">
            <div className="gha-merge-row">
                <StatusMark outcome={outcome} size={14} />
                <span className="gha-merge-title">{title}</span>
                {detail && <span className="gha-merge-detail">{detail}</span>}
            </div>
        </div>
    );
}

function MergeBox({ repo, view, active, onOpenRun }: { repo: RepoRef; view: OpenPull; active: boolean; onOpenRun: (runId: string) => void }) {
    const { pull, reviewed } = view;
    const verdict = view.verdict ? VERDICT_LABEL[view.verdict] : null;
    const host = useHost();
    const { pulls } = host.capabilities;
    const methods = MERGE_METHODS.filter((each) => pulls.mergeMethods.includes(each.value));
    const [method, setMethod] = useState<MergeMethod>(methods[0]?.value ?? "merge");
    const now = useNow(false);
    const [busy, runBusy] = useBusy();
    const base = pull.base ?? "the base branch";

    const merge = async () => {
        const sure = await confirmDialog({
            title: `Merge #${pull.number} into ${base}?`,
            body: pull.title,
            confirmLabel: "Merge",
        });
        if (!sure) return;
        try {
            await hostApi(repo.provider).mergePull(repo, pull.number, method, pull.headSha ?? "");
            notify("success", `Merged #${pull.number}`);
        } catch (error) {
            reportError(`Could not merge #${pull.number}`)(error);
        }
    };

    const open = pull.state === "open";
    const setState = async () => {
        if (open) {
            const sure = await confirmDialog({
                title: `Close #${pull.number} without merging?`,
                body: pull.title,
                confirmLabel: "Close pull request",
                destructive: true,
            });
            if (!sure) return;
        }
        try {
            await hostApi(repo.provider).setPullState(repo, pull.number, open ? "closed" : "open");
            notify("success", open ? `Closed #${pull.number}` : `Reopened #${pull.number}`);
        } catch (error) {
            reportError(open ? "Could not close it" : "Could not reopen it")(error);
        }
    };

    const checks = <PullChecks repo={repo} checks={view.checks} runs={view.runs} active={active} onOpenRun={onOpenRun} />;
    if (pull.state === "merged") {
        const merger = pull.mergedBy ?? null;
        const commit = pull.mergeCommitSha;
        return (
            <div className="gha-merge-box" data-state="merged">
                <div className="gha-merge-part">
                    <div className="gha-merge-row">
                        <StateMark kind="pull" state="merged" />
                        <span className="gha-merge-title">Merged into</span>
                        {pull.base && <Branch name={pull.base} />}
                    </div>
                    <div className="gha-merge-by">
                        {merger && <Who login={merger} avatarUrl={pull.avatars[merger] ?? (merger === pull.author ? pull.avatarUrl : null)} />}
                        <span>{formatAgo(pull.mergedAt, now)}</span>
                        {commit && (
                            <Tooltip label="Copy the merge commit">
                                <button
                                    type="button"
                                    className="gha-link gha-mono"
                                    onClick={() =>
                                        void copyText(commit)
                                            .then(() => notify("success", `Copied ${commit.slice(0, 7)}`))
                                            .catch(swallow("copy the commit"))
                                    }>
                                    {commit.slice(0, 7)}
                                </button>
                            </Tooltip>
                        )}
                    </div>
                </div>
                {checks}
            </div>
        );
    }
    if (!open) {
        return (
            <div className="gha-merge-box">
                <div className="gha-merge-part">
                    <div className="gha-merge-row">
                        <StateMark kind="pull" state="closed" />
                        <span className="gha-merge-title">Closed without merging</span>
                        <span className="gha-page-spacer" />
                        {pulls.reopen && (
                            <button type="button" className="gha-btn" disabled={busy} onClick={() => runBusy(setState)}>
                                Reopen
                            </button>
                        )}
                    </div>
                </div>
                {checks}
            </div>
        );
    }

    const verdictPart =
        verdict === "Approved"
            ? { outcome: "success" as const, title: "Approved" }
            : verdict === "Changes requested"
              ? { outcome: "failure" as const, title: "Changes requested" }
              : { outcome: "queued" as const, title: reviewed ? "No approving review yet" : "No reviews yet" };
    const merging = pulls.mergeability ? mergeability(view.merge, base, host.name) : null;
    return (
        <div className="gha-merge-box">
            <MergePart outcome={verdictPart.outcome} title={verdictPart.title} />
            {checks}
            {pull.draft ? (
                <MergePart outcome="queued" title="This is a draft" detail={`Mark it ready for review on ${host.name} before merging.`} />
            ) : (
                merging && <MergePart outcome={merging.outcome} title={merging.title} detail={merging.detail} />
            )}
            <div className="gha-merge-actions">
                {!pull.draft && (
                    <>
                        <Dropdown value={method} options={methods} onChange={(value) => setMethod(value as MergeMethod)} title="How to merge" />
                        <button
                            type="button"
                            className="gha-btn primary"
                            disabled={busy || pull.mergeState === "dirty"}
                            onClick={() => runBusy(merge)}>
                            Merge pull request
                        </button>
                    </>
                )}
                <span className="gha-page-spacer" />
                <button type="button" className="gha-btn danger" disabled={busy} onClick={() => runBusy(setState)}>
                    Close
                </button>
            </div>
        </div>
    );
}

/** Checks the pull request's branch out in this project, one of the side panel's actions. */
function CheckoutButton({
    cwd,
    project,
    pull,
    checkout,
    current,
}: {
    cwd: string | null;
    project: string | null;
    pull: Pull;
    /** The local branch the core says checking it out makes, none when it cannot be. */
    checkout: string | null;
    current: string | null;
}) {
    const [busy, runBusy] = useBusy();
    const here = !!checkout && checkout === current;
    const can = !!cwd && !!project && pull.state === "open" && !!checkout;
    return (
        <Tooltip
            label={
                here
                    ? `${checkout} is checked out in this project`
                    : can
                      ? `Check out ${checkout} in this project`
                      : "Only an open pull request in this project's repository can be checked out"
            }>
            <button
                type="button"
                className="git-btn"
                disabled={busy || here || !can}
                onClick={() =>
                    project &&
                    runBusy(() =>
                        checkoutPull(project, pull.number)
                            .then(() => notify("success", `Checked out #${pull.number}`))
                            .catch(reportError(`Could not check out #${pull.number}`)),
                    )
                }>
                {here ? <IconCheck size={13} /> : <IconCheckout size={13} />}
                {here ? "Checked out" : busy ? "Checking out…" : "Check out"}
            </button>
        </Tooltip>
    );
}

/** The pull request's changed files as rows, like Changes lists staged and unstaged ones. */
function PullFileRows({
    project,
    repo,
    number,
    active,
    focus,
    onFocus,
}: {
    project: string | null;
    repo: RepoRef;
    number: number;
    active: boolean;
    focus: string | null;
    onFocus: (path: string) => void;
}) {
    const files = useResourceEnabled(active, pullFilesR, project, repo, number);
    const list = files.data ?? [];
    const added = list.reduce((sum, file) => sum + file.additions, 0);
    const removed = list.reduce((sum, file) => sum + file.deletions, 0);
    const [folded, setFolded] = useState(false);
    return (
        <div className="git-list pr-files">
            <div className="git-group git-file-group">
                <button type="button" className="git-group-toggle" aria-expanded={!folded} onClick={() => setFolded((was) => !was)}>
                    <IconChevron size={13} className="git-group-chev" />
                    <span className="git-group-icon">
                        <IconFile size={13} />
                    </span>
                    <span className="git-group-name">Files</span>
                </button>
                {list.length > 0 && (
                    <span className="gha-diffstat pr-files-stat">
                        <span className="gha-add">+{added.toLocaleString()}</span>
                        <span className="gha-del">−{removed.toLocaleString()}</span>
                    </span>
                )}
                {list.length > 0 && <span className="git-group-count">{list.length}</span>}
            </div>
            {files.status === "loading" && !files.data && <SkeletonRows rows={3} label="Loading files" />}
            {!folded &&
                list.map((file, index) => {
                    const badge = hostFileStatus(file.status);
                    const dir = dirname(file.path);
                    return (
                        <button
                            key={file.path}
                            type="button"
                            className={`git-row git-tree-row git-file-row${focus === file.path ? " sel" : ""}`}
                            data-first={index === 0 ? "" : undefined}
                            data-last={index === list.length - 1 ? "" : undefined}
                            title={file.path}
                            onClick={() => onFocus(file.path)}>
                            <FileIcon name={basename(file.path)} size={14} />
                            <span className="git-row-name">
                                {basename(file.path)}
                                {dir && <span className="git-row-dir">{dir}</span>}
                            </span>
                            <span className={`git-status git-${badge.cls}`} title={badge.label}>
                                {badge.letter}
                            </span>
                        </button>
                    );
                })}
        </div>
    );
}

export type PullTab = "conversation" | "files";

/**
 * The right column of an open pull request: a header like a commit's in Changes, with a toggle where a commit's
 * actions sit, between the conversation and every changed file's diff.
 */
export function PullRight({
    repo,
    project,
    cwd,
    number,
    tab,
    commit,
    focus,
    login,
    active,
    onTab,
    onLeaveCommit,
}: {
    repo: RepoRef;
    /** The project the pull request is of, or none for one the inbox opened. */
    project: string | null;
    cwd: string | null;
    number: number;
    tab: PullTab;
    commit: string | null;
    focus: string | null;
    login: string | null;
    active: boolean;
    onTab: (tab: PullTab) => void;
    onLeaveCommit: () => void;
}) {
    const host = useHost();
    const view = useResourceEnabled(active, pullViewR, project, repo, number);
    const timeline = useResourceEnabled(active, timelineR, repo, number, "pull");
    const now = useNow(false);
    if (!view.data) return <SkeletonRows rows={6} label="Loading pull request" />;
    const found = view.data.pull;
    const said =
        (timeline.data ?? []).filter((item) => item.kind === "commented" || (item.kind === "reviewed" && (item.body ?? "").trim())).length + 1;
    const tabs: { id: PullTab; label: string; count: number | null }[] = [
        { id: "conversation", label: "Conversation", count: said },
        { id: "files", label: "Files changed", count: found.changedFiles },
    ];
    return (
        <div className="pr-right">
            <div className="git-detail">
                <div className="pr-headline">
                    <PullHeadline
                        pull={found}
                        now={now}
                        labels
                        title={<h2 className="git-detail-title pr-headline-title">{found.title}</h2>}
                        trailing={
                            <Tooltip label={`Open on ${host.name}`}>
                                <button
                                    type="button"
                                    className="gha-icon-btn"
                                    aria-label={`Open on ${host.name}`}
                                    onClick={() => void openUrl(found.url).catch(swallow(`open ${host.name}`))}>
                                    <IconExternal size={12} />
                                </button>
                            </Tooltip>
                        }
                    />
                </div>
                <div className="git-detail-actions" role="tablist" aria-label="Pull request">
                    {tabs.map((each) => (
                        <button
                            key={each.id}
                            type="button"
                            role="tab"
                            aria-selected={tab === each.id}
                            className="gha-chip pr-tab"
                            data-on={tab === each.id ? "1" : "0"}
                            onClick={() => onTab(each.id)}>
                            {each.label}
                            {each.count !== null && <span className="gha-tab-count">{each.count.toLocaleString()}</span>}
                        </button>
                    ))}
                    {tab === "files" && commit && (
                        <>
                            <span className="gha-page-spacer" />
                            <span className="gha-dim">
                                Commit <span className="gha-mono">{commit.slice(0, 7)}</span>
                            </span>
                            <button type="button" className="gha-chip" onClick={onLeaveCommit}>
                                All changes
                            </button>
                        </>
                    )}
                </div>
            </div>
            {tab === "conversation" ? (
                <div className="pr-conversation">
                    <CommentThread
                        repo={repo}
                        number={found.number}
                        active={active}
                        now={now}
                        opening={{
                            key: "opening",
                            author: found.author,
                            avatarUrl: found.avatarUrl,
                            association: found.authorAssociation,
                            at: found.createdAt,
                            body: found.body,
                            review: null,
                        }}
                        base={found.base}
                        review={found.state === "open" ? { mine: !!login && found.author === login } : null}
                    />
                </div>
            ) : commit ? (
                cwd ? (
                    <CommitReview
                        key={commit}
                        repo={cwd}
                        rev={commit}
                        title={commit.slice(0, 7)}
                        subtitle=""
                        head={<></>}
                        onOpenFile={requestOpenFile}
                    />
                ) : (
                    <EmptyState message="This repository is not checked out here, so its commits cannot be opened." />
                )
            ) : (
                <PullFiles project={project} repo={repo} pull={found} cwd={cwd} active={active} focusPath={focus ?? undefined} />
            )}
        </div>
    );
}

/** A pull request's commits, as the git pane's own history: newest first, one lane, the same rows. */
function PullHistory({
    repo,
    number,
    active,
    selected,
    onSelect,
}: {
    repo: RepoRef;
    number: number;
    active: boolean;
    selected: string | null;
    onSelect: (sha: string | null) => void;
}) {
    const host = useHost();
    const commits = useResourceEnabled(active, pullCommitsR, repo, number);
    const now = useNow(false);
    // The host names each commit's author rather than their email, so the graph finds their picture by that name.
    const pictures = useMemo<AuthorPictures>(() => {
        const byAuthor = new Map<string, string | null>();
        for (const commit of commits.data ?? []) {
            if (commit.author && !byAuthor.get(commit.author))
                byAuthor.set(commit.author, commit.avatarUrl ?? host.avatarForLogin?.(commit.author) ?? null);
        }
        return { pictureFor: (login) => byAuthor.get(login) ?? null, load: (url) => host.api.image(url) };
    }, [commits.data, host]);
    const rows = useMemo<GitCommit[]>(() => {
        const list = [...(commits.data ?? [])].reverse();
        return list.map((commit, index) => ({
            hash: commit.sha.slice(0, 7),
            full_hash: commit.sha,
            parents: list[index + 1] ? [list[index + 1].sha] : [],
            author: commit.author ?? "someone",
            author_email: commit.author ?? "",
            date: formatAgo(commit.date, now),
            subject: commit.message.split("\n")[0],
            refs: [],
            unpushed: false,
        }));
    }, [commits.data, now]);
    const index = selected ? rows.findIndex((row) => row.full_hash === selected) : -1;
    return (
        <div className="git-list">
            {commits.status === "loading" && !commits.data ? (
                <SkeletonRows rows={4} label="Loading commits" />
            ) : (
                <AuthorPicturesProvider value={pictures}>
                    <GitGraph
                        commits={rows}
                        selectedIndex={index}
                        focused={index >= 0}
                        range={null}
                        onSelect={(i) => onSelect(rows[i]?.full_hash ?? null)}
                        onActivate={() => {}}
                    />
                </AuthorPicturesProvider>
            )}
        </div>
    );
}

type SidePage = "files" | "commits" | "merge";

/** An open pull request's side panel, laid out like the git pane's: what it is and its actions, then its files, commits and merge as pages. */
function PullColumn({
    repo,
    project,
    cwd,
    projectBranch,
    number,
    active,
    focus,
    commit,
    onFocus,
    onCommit,
    onClose,
    onOpenRun,
}: DetailProps & {
    focus: string | null;
    commit: string | null;
    onFocus: (path: string) => void;
    onCommit: (sha: string | null) => void;
    onClose: () => void;
}) {
    const host = useHost();
    const view = useResourceEnabled(active, pullViewR, project, repo, number);
    const [page, setPage] = useState<SidePage>("files");
    if (view.status === "loading" && !view.data) return <SkeletonRows rows={8} label="Loading pull request" />;
    if (!view.data) {
        return <EmptyState title="Could not read it" message={errorText(view.error)} tone="error" action={{ label: "Back", onClick: onClose }} />;
    }
    const found = view.data.pull;
    const merge = (
        <div className="pr-card">
            <MergeBox repo={repo} view={view.data} active={active} onOpenRun={onOpenRun} />
        </div>
    );
    const open = found.state === "open";
    return (
        <div className="git-side">
            <div className="git-side-head">
                <div className="git-side-title">
                    <Tooltip label="Back to the pull requests">
                        <button type="button" className="git-foot-btn icon pr-back-btn" aria-label="Back to the pull requests" onClick={onClose}>
                            <IconChevron size={11} />
                        </button>
                    </Tooltip>
                    <StateMark kind="pull" state={view.data.state} size={14} />
                    <span className="pr-side-number">#{found.number}</span>
                    <span className="gha-page-spacer" />
                    <ShellAccount />
                </div>
                <div className="git-btn-group git-side-actions pr-side-actions" role="group" aria-label="Pull request">
                    <CheckoutButton cwd={cwd} project={project} pull={found} checkout={view.data.checkout ?? null} current={projectBranch} />
                    <button type="button" className="git-btn" onClick={() => void openUrl(found.url).catch(swallow(`open ${host.name}`))}>
                        <IconExternal size={13} />
                        {host.name}
                    </button>
                    <button
                        type="button"
                        className={`git-btn${open && !found.draft && found.mergeState !== "dirty" ? " primary" : ""}`}
                        onClick={() => setPage("merge")}>
                        <IconMerge size={13} />
                        {open ? "Merge" : view.data.state === "merged" ? "Merged" : "Closed"}
                    </button>
                </div>
            </div>
            <SwipePages
                label="Pull request"
                current={page}
                onChange={(id) => setPage(id as SidePage)}
                pages={[
                    {
                        id: "files",
                        label: "Files",
                        icon: <IconFile size={14} />,
                        content: <PullFileRows project={project} repo={repo} number={number} active={active} focus={focus} onFocus={onFocus} />,
                    },
                    {
                        id: "commits",
                        label: "Commits",
                        icon: <IconCommit size={14} />,
                        content: <PullHistory repo={repo} number={number} active={active} selected={commit} onSelect={onCommit} />,
                    },
                    { id: "merge", label: "Checks and merge", icon: <IconCheckCircle size={14} />, content: merge },
                ]}
            />
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
    projectBranch: string | null;
    /** The project folder, when this is its own repository. */
    cwd: string | null;
    login: string | null;
    active: boolean;
}

export function PullsView({ paneId, repo, project, listState, item, composing, projectBranch, cwd, login, active }: Props) {
    const listing = item === null && !composing;
    const pulls = useResourceEnabled(active, pullListR, project, listState);
    const counts = pulls.data?.counts;
    const open = useResourceEnabled(active && listing && !!projectBranch, pullListR, project, "open");
    useCiWatch(repo, active && listing);
    const now = useNow(false);
    const [commit, setCommit] = useState<{ pull: number; sha: string } | null>(null);
    const [focus, setFocus] = useState<{ pull: number; path: string } | null>(null);
    const shownCommit = commit && commit.pull === item ? commit.sha : null;
    const focusPath = focus && focus.pull === item ? focus.path : null;
    const [tabOf, setTabOf] = useState<{ pull: number; tab: PullTab } | null>(null);
    const tab: PullTab = tabOf && tabOf.pull === item ? tabOf.tab : "conversation";

    if (composing) {
        return (
            <NewPullForm
                paneId={paneId}
                repo={repo}
                project={project}
                cwd={cwd}
                active={active}
                onCreated={(number) => showItem(paneId, number)}
                onCancel={() => compose(paneId, null)}
            />
        );
    }

    const rows = pulls.data?.pulls ?? [];
    const offer =
        !!open.data &&
        needsPull(
            projectBranch,
            open.data.pulls.map(({ pull }) => pull),
            [],
        );
    const list =
        pulls.status === "loading" && !pulls.data ? (
            <SkeletonRows rows={8} label="Loading pull requests" />
        ) : pulls.error && !pulls.data ? (
            <EmptyState
                title="Could not read pull requests"
                message={errorText(pulls.error)}
                tone="error"
                action={{ label: "Try again", onClick: () => void pulls.refresh() }}
            />
        ) : (
            <div className="gha-list pr-list">
                <StaleNotice handle={pulls} />
                <div className="gha-list-head">
                    <ListTitle section="pulls" />
                    <Segmented
                        label="Which to list"
                        value={listState}
                        options={LIST_STATES.map((state) => ({
                            value: state,
                            label: state === "all" ? "All" : state === "open" ? "Open" : "Closed",
                            count: counts && state !== "all" ? counts[state as "open" | "closed"] : undefined,
                        }))}
                        onChange={(state) => setListState(paneId, "pulls", state)}
                    />
                    <span className="gha-page-spacer" />
                    <Tooltip label="New pull request">
                        <button type="button" className="gha-icon-btn" aria-label="New pull request" onClick={() => compose(paneId, "pull")}>
                            <IconPlus size={13} />
                        </button>
                    </Tooltip>
                    <ShellAccount />
                </div>
                {offer && projectBranch && (
                    <div className="gha-callout">
                        <span>
                            <span className="gha-tag">{projectBranch}</span> has no pull request yet.
                        </span>
                        <button type="button" className="gha-btn primary" onClick={() => compose(paneId, "pull")}>
                            Open one
                        </button>
                    </div>
                )}
                {rows.length === 0 ? (
                    <EmptyState icon={<IconPullRequest size={20} />} message={`No ${listState === "all" ? "" : listState} pull requests.`} />
                ) : (
                    <div className="pr-rows">
                        {rows.map((listed) => (
                            <PullRow key={listed.pull.number} listed={listed} now={now} onOpen={() => showItem(paneId, listed.pull.number)} />
                        ))}
                    </div>
                )}
            </div>
        );

    if (item === null) return <div className="gha-full-page">{list}</div>;

    const left = (
        <PullColumn
            repo={repo}
            project={project}
            cwd={cwd}
            projectBranch={projectBranch}
            number={item}
            active={active}
            login={login}
            focus={focusPath}
            commit={shownCommit}
            onFocus={(path) => {
                setCommit(null);
                setFocus({ pull: item, path });
                setTabOf({ pull: item, tab: "files" });
            }}
            onCommit={(sha) => {
                setCommit(sha ? { pull: item, sha } : null);
                if (sha) setTabOf({ pull: item, tab: "files" });
            }}
            onClose={() => showItem(paneId, null)}
            onBack={() => showItem(paneId, null)}
            onOpenRun={(runId) => openRunFrom(paneId, runId, item)}
        />
    );

    const right = (
        <PullRight
            repo={repo}
            project={project}
            cwd={cwd}
            number={item}
            tab={tab}
            commit={shownCommit}
            focus={focusPath}
            login={login}
            active={active}
            onTab={(next) => setTabOf({ pull: item, tab: next })}
            onLeaveCommit={() => setCommit(null)}
        />
    );

    return <GitColumns paneId={paneId} left={left} right={<div className="git-right-review">{right}</div>} />;
}
