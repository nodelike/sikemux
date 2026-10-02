import { useMemo, useState, type ReactNode } from "react";
import { confirmDialog, copyText, notify, openUrl, reportError, swallow } from "../../plugin-api/host";
import { checkoutPull, isOwnBranch, localBranchOf } from "../checkout";
import { useHost } from "../registry";
import { invalidate, useResourceEnabled } from "../../plugin-api/resources";
import {
    Dropdown,
    EmptyState,
    IconCheck,
    IconCheckout,
    IconChevron,
    IconExternal,
    IconPlus,
    IconPullRequest,
    SkeletonRows,
    Tooltip,
} from "../../plugin-api/ui";
import type { GitCommit } from "../../api/git";
import { CommitReview } from "../../git/CommitReview";
import { GitColumns } from "../../git/GitColumns";
import { AuthorPicturesProvider, type AuthorPictures } from "../../git/AuthorAvatar";
import { GitGraph } from "../../git/GitGraph";
import { FoldPanel } from "../../git/FoldPanel";
import { FileIcon } from "../../ui/FileIcon";
import { basename, dirname } from "../../lib/paths";
import { requestOpenFile, setGitView } from "../../state/commands";
import { useStore } from "../../state/store";
import { hostApi, failureMessage, type MergeMethod, type Pull, type RepoRef, type Run } from "../api";
import { pullCommitsR, pullFilesR, pullR, pullReviewsR, pullsR, runsR, timelineR } from "../resources";
import { checksSummary, formatAgo, isUnfinished, OUTCOME_LABEL, overallOutcome, type Outcome } from "../runStatus";
import { needsPull } from "../compose";
import { compose, openRunFrom, setListState, showItem } from "../state";
import { CommentThread, Face } from "./CommentThread";
import { OutcomeIcon } from "./ActionsIcon";
import { Branch, Comments, Labels, StateMark, stateLabel, stateOf, Who } from "./Bits";
import { useBusy, useEvery, useNow } from "./hooks";
import { NewPullForm } from "./NewPullForm";
import { PullFiles } from "./PullFiles";
import { PullChecks } from "./PullChecks";

const LIST_STATES = ["open", "closed", "all"];
const LIVE_REFRESH_MS = 10_000;

export function reviewVerdict(reviews: readonly { author: string | null; state: string }[]): string | null {
    // Only a person's latest review counts, which is how GitHub scores it too.
    const latest = new Map<string, string>();
    for (const review of reviews) {
        if (review.state === "COMMENTED" || review.state === "DISMISSED") continue;
        latest.set(review.author ?? "", review.state);
    }
    const states = [...latest.values()];
    if (states.includes("CHANGES_REQUESTED")) return "Changes requested";
    if (states.includes("APPROVED")) return "Approved";
    return null;
}

/** Who opened it, its state and title, then its number, age and branches: one headline for the list and the open pull request. */
function PullHeadline({
    pull,
    now,
    title,
    trailing,
    labels = false,
}: {
    pull: Pull;
    now: number;
    title: ReactNode;
    trailing?: ReactNode;
    labels?: boolean;
}) {
    const state = stateOf(pull.state, pull.draft);
    return (
        <>
            <span className="pr-face">
                <Face login={pull.author} url={pull.avatarUrl} />
            </span>
            <span className="pr-headline-top">
                <span className="gha-state-word pr-state" data-kind="pull" data-state={state}>
                    {stateLabel("pull", pull.state, pull.draft)}
                </span>
                {title}
                {trailing}
            </span>
            <span className="pr-headline-sub">
                <span className="pr-headline-who">
                    {pull.author ?? "someone"} · #{pull.number}
                    <span className="pr-headline-ago">
                        {labels ? ", updated " : " · "}
                        {formatAgo(pull.updatedAt, now)}
                    </span>
                </span>
                {pull.head && <span className="pr-ref pr-ref-head">{pull.head}</span>}
                {pull.head && pull.base && <span className="pr-ref-arrow">→</span>}
                {pull.base && <span className="pr-ref">{pull.base}</span>}
                {labels && <Labels labels={pull.labels} />}
            </span>
        </>
    );
}

function PullCi({ runs }: { runs: readonly Run[] }) {
    const outcome = overallOutcome(runs);
    const summary = checksSummary(runs);
    return (
        <Tooltip label={summary === "all passed" ? "All checks passed" : `Checks: ${summary}`}>
            <span className="pr-ci" data-outcome={outcome}>
                <OutcomeIcon outcome={outcome} size={12} />
                <span className="pr-ci-label">{OUTCOME_LABEL[outcome]}</span>
            </span>
        </Tooltip>
    );
}

function PullRow({ pull, runs, now, onOpen }: { pull: Pull; runs: readonly Run[] | undefined; now: number; onOpen: () => void }) {
    const ci = runs ? <PullCi runs={runs} /> : null;
    return (
        <button type="button" className="pr-row pr-headline" onClick={onOpen}>
            <PullHeadline
                pull={pull}
                now={now}
                title={<span className="pr-headline-title">{pull.title}</span>}
                trailing={
                    <>
                        {ci}
                        <Comments count={pull.comments ?? 0} />
                    </>
                }
            />
            <span className="pr-row-ci">{ci}</span>
            <span className="pr-row-reviewers">
                {pull.reviewers.map((login) => (
                    <Face key={login} login={login} url={pull.avatars[login] ?? null} />
                ))}
            </span>
            <span className="pr-row-comments">
                <Comments count={pull.comments ?? 0} />
            </span>
            <span className="pr-row-updated">{formatAgo(pull.updatedAt, now)}</span>
        </button>
    );
}

interface DetailProps {
    repo: RepoRef;
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

function mergeability(mergeState: string | null, base: string): { outcome: Outcome; title: string; detail: string | null } {
    switch (mergeState) {
        case "dirty":
            return { outcome: "failure", title: `Conflicts with ${base}`, detail: "Resolve the conflicts before merging." };
        case "blocked":
            return { outcome: "blocked", title: "Merging is blocked", detail: "GitHub is waiting on the required reviews and checks." };
        case "behind":
            return { outcome: "blocked", title: `Behind ${base}`, detail: "Bring the branch up to date before merging." };
        case "clean":
        case "unstable":
        case "has_hooks":
            return { outcome: "success", title: "Ready to merge", detail: null };
        default:
            return { outcome: "queued", title: "Checking whether it can merge", detail: null };
    }
}

function MergePart({ outcome, title, detail }: { outcome: Outcome; title: string; detail?: string | null }) {
    return (
        <div className="gha-merge-part">
            <div className="gha-merge-row">
                <OutcomeIcon outcome={outcome} size={12} />
                <span className="gha-merge-title">{title}</span>
                {detail && <span className="gha-merge-detail">{detail}</span>}
            </div>
        </div>
    );
}

function MergeBox({
    repo,
    pull,
    verdict,
    reviewed,
    active,
    onOpenRun,
    checkout,
}: {
    repo: RepoRef;
    pull: Pull;
    verdict: string | null;
    reviewed: boolean;
    active: boolean;
    onOpenRun: (runId: string) => void;
    /** Checking the branch out here, beside Close. */
    checkout?: ReactNode;
}) {
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
            invalidate((kind) => kind.startsWith("host.pull"));
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
            invalidate((kind) => kind.startsWith("host.pull"));
        } catch (error) {
            reportError(open ? "Could not close it" : "Could not reopen it")(error);
        }
    };

    const checks = pull.headSha ? <PullChecks repo={repo} sha={pull.headSha} active={active} onOpenRun={onOpenRun} /> : null;
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
    const merging = pulls.mergeability ? mergeability(pull.mergeState, base) : null;
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
                {checkout}
                <button type="button" className="gha-btn danger" disabled={busy} onClick={() => runBusy(setState)}>
                    Close
                </button>
            </div>
        </div>
    );
}

function CheckoutButton({ cwd, repo, pull, current }: { cwd: string; repo: RepoRef; pull: Pull; current: string | null }) {
    const host = useHost();
    const [busy, runBusy] = useBusy();
    const local = localBranchOf(pull, repo);
    if (local && local === current) {
        return (
            <span className="gha-checked-out" title={`${local} is checked out in this project`}>
                <IconCheck size={12} /> Checked out
            </span>
        );
    }
    if (!isOwnBranch(pull, repo) && !host.pullHeadRef) return null;
    return (
        <Tooltip label={`Check out ${local ?? "the branch"} in this project`}>
            <button
                type="button"
                className="gha-btn"
                disabled={busy}
                onClick={() =>
                    runBusy(() =>
                        checkoutPull(cwd, host, repo, pull)
                            .then(() => notify("success", `Checked out #${pull.number}`))
                            .catch(swallow("check out the pull request")),
                    )
                }>
                <IconCheckout size={12} /> {busy ? "Checking out…" : "Check out"}
            </button>
        </Tooltip>
    );
}

/** The top of an open pull request's column, where Changes has its commit box: what it is and what stands between it and merged. */
function PullCard({
    repo,
    cwd,
    projectBranch,
    pull,
    verdict,
    reviewed,
    active,
    onClose,
    onOpenRun,
}: {
    repo: RepoRef;
    cwd: string | null;
    projectBranch: string | null;
    pull: Pull;
    verdict: string | null;
    reviewed: boolean;
    active: boolean;
    onClose: () => void;
    onOpenRun: (runId: string) => void;
}) {
    return (
        <>
            <button type="button" className="pr-back" onClick={onClose}>
                <IconChevron size={11} /> Pull requests
            </button>
            <div className="pr-card">
                <MergeBox
                    repo={repo}
                    pull={pull}
                    verdict={verdict}
                    reviewed={reviewed}
                    active={active}
                    onOpenRun={onOpenRun}
                    checkout={cwd && pull.state === "open" ? <CheckoutButton cwd={cwd} repo={repo} pull={pull} current={projectBranch} /> : null}
                />
            </div>
        </>
    );
}

const FILE_STATUS: Record<string, { letter: string; cls: string }> = {
    added: { letter: "A", cls: "added" },
    removed: { letter: "D", cls: "deleted" },
    renamed: { letter: "R", cls: "renamed" },
    copied: { letter: "C", cls: "renamed" },
};

/** The pull request's changed files as rows, like Changes lists staged and unstaged ones. */
function PullFileRows({
    repo,
    number,
    active,
    focus,
    onFocus,
}: {
    repo: RepoRef;
    number: number;
    active: boolean;
    focus: string | null;
    onFocus: (path: string) => void;
}) {
    const files = useResourceEnabled(active, pullFilesR, repo, number);
    const list = files.data ?? [];
    const added = list.reduce((sum, file) => sum + file.additions, 0);
    const removed = list.reduce((sum, file) => sum + file.deletions, 0);
    return (
        <div className="git-list pr-files">
            <div className="git-group git-file-group">
                <span className="git-label">Files</span>
                {list.length > 0 && <span className="git-count">{list.length}</span>}
                {list.length > 0 && (
                    <span className="gha-diffstat pr-files-stat">
                        <span className="gha-add">+{added.toLocaleString()}</span>
                        <span className="gha-del">−{removed.toLocaleString()}</span>
                    </span>
                )}
            </div>
            {files.status === "loading" && !files.data && <SkeletonRows rows={3} label="Loading files" />}
            {list.map((file) => {
                const badge = FILE_STATUS[file.status] ?? { letter: "M", cls: "modified" };
                const dir = dirname(file.path);
                return (
                    <button
                        key={file.path}
                        type="button"
                        className={`git-row git-file-row${focus === file.path ? " sel" : ""}`}
                        title={file.path}
                        onClick={() => onFocus(file.path)}>
                        <FileIcon name={basename(file.path)} size={14} />
                        <span className="git-row-name">
                            {basename(file.path)}
                            {dir && <span className="git-row-dir">{dir}</span>}
                        </span>
                        <span className={`git-status ${badge.cls}`}>{badge.letter}</span>
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
    const pull = useResourceEnabled(active, pullR, repo, number);
    const timeline = useResourceEnabled(active, timelineR, repo, number);
    const now = useNow(false);
    if (!pull.data) return <SkeletonRows rows={6} label="Loading pull request" />;
    const found = pull.data;
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
                        withoutCommits
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
                <PullFiles repo={repo} pull={found} cwd={cwd} active={active} focusPath={focus ?? undefined} />
            )}
        </div>
    );
}

/** A pull request's commits, as the git pane's own history: newest first, one lane, the same rows. */
function PullHistory({
    paneId,
    repo,
    number,
    active,
    selected,
    onSelect,
}: {
    paneId: string;
    repo: RepoRef;
    number: number;
    active: boolean;
    selected: string | null;
    onSelect: (sha: string | null) => void;
}) {
    const open = useStore((s) => s.gitViews[paneId]?.historyOpen ?? false);
    const height = useStore((s) => s.gitViews[paneId]?.historyHeight ?? null);
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
        <FoldPanel
            label="Commits"
            count={rows.length}
            summary={rows[0]?.subject ?? null}
            open={open}
            height={height}
            onToggle={() => {
                if (open) onSelect(null);
                setGitView(paneId, { historyOpen: !open });
            }}
            onResize={(next) => setGitView(paneId, { historyHeight: next })}>
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
        </FoldPanel>
    );
}

/** An open pull request, laid out like Changes: its card, its files, then its conversation and commits folding beneath. */
function PullColumn({
    paneId,
    repo,
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
    paneId: string;
    focus: string | null;
    commit: string | null;
    onFocus: (path: string) => void;
    onCommit: (sha: string | null) => void;
    onClose: () => void;
}) {
    const pull = useResourceEnabled(active, pullR, repo, number);
    const reviews = useResourceEnabled(active, pullReviewsR, repo, number);
    if (pull.status === "loading" && !pull.data) return <SkeletonRows rows={8} label="Loading pull request" />;
    if (!pull.data) {
        return (
            <EmptyState title="Could not read it" message={failureMessage(pull.error)} tone="error" action={{ label: "Back", onClick: onClose }} />
        );
    }
    const found = pull.data;
    const reviewList = reviews.data ?? [];
    const verdict = reviewVerdict(reviewList);
    return (
        <>
            <PullCard
                repo={repo}
                cwd={cwd}
                projectBranch={projectBranch}
                pull={found}
                verdict={verdict}
                reviewed={reviewList.length > 0}
                active={active}
                onClose={onClose}
                onOpenRun={onOpenRun}
            />
            <PullFileRows repo={repo} number={number} active={active} focus={focus} onFocus={onFocus} />
            <PullHistory paneId={paneId} repo={repo} number={number} active={active} selected={commit} onSelect={onCommit} />
        </>
    );
}

interface Props {
    paneId: string;
    repo: RepoRef;
    listState: string;
    item: number | null;
    composing: boolean;
    projectBranch: string | null;
    /** The project folder, when this is its own repository. */
    cwd: string | null;
    login: string | null;
    active: boolean;
}

export function PullsView({ paneId, repo, listState, item, composing, projectBranch, cwd, login, active }: Props) {
    const listing = item === null && !composing;
    const pulls = useResourceEnabled(active, pullsR, repo, listState);
    const open = useResourceEnabled(active && listing && !!projectBranch, pullsR, repo, "open");
    const recentRuns = useResourceEnabled(active && listing, runsR, { ...repo, perPage: 100 });
    const runsBySha = useMemo(() => {
        const bySha = new Map<string, Run[]>();
        for (const run of recentRuns.data?.runs ?? []) bySha.set(run.sha, [...(bySha.get(run.sha) ?? []), run]);
        return bySha;
    }, [recentRuns.data]);
    const runsLive = active && listing && (recentRuns.data?.runs.some(isUnfinished) ?? false);
    useEvery(runsLive, LIVE_REFRESH_MS, () => void recentRuns.refresh());
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
                cwd={cwd}
                head={projectBranch}
                active={active}
                onCreated={(number) => showItem(paneId, number)}
                onCancel={() => compose(paneId, null)}
            />
        );
    }

    const rows = pulls.data ?? [];
    const offer = !!open.data && needsPull(projectBranch, open.data, []);
    const list =
        pulls.status === "loading" && !pulls.data ? (
            <SkeletonRows rows={8} label="Loading pull requests" />
        ) : pulls.error ? (
            <EmptyState
                title="Could not read pull requests"
                message={failureMessage(pulls.error)}
                tone="error"
                action={{ label: "Try again", onClick: () => void pulls.refresh() }}
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
                                onClick={() => setListState(paneId, "pulls", state)}>
                                {state === "all" ? "All" : state === "open" ? "Open" : "Closed"}
                            </button>
                        ))}
                    </div>
                    <span className="gha-page-spacer" />
                    <Tooltip label="New pull request">
                        <button type="button" className="gha-icon-btn" aria-label="New pull request" onClick={() => compose(paneId, "pull")}>
                            <IconPlus size={13} />
                        </button>
                    </Tooltip>
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
                        <div className="pr-cols" aria-hidden="true">
                            <span>Pull request</span>
                            <span>CI</span>
                            <span>Reviewers</span>
                            <span>Comments</span>
                            <span>Updated</span>
                        </div>
                        {rows.map((pull) => (
                            <PullRow
                                key={pull.number}
                                pull={pull}
                                runs={pull.headSha ? runsBySha.get(pull.headSha) : undefined}
                                now={now}
                                onOpen={() => showItem(paneId, pull.number)}
                            />
                        ))}
                    </div>
                )}
            </div>
        );

    if (item === null) return <div className="gha-full-page">{list}</div>;

    const left = (
        <PullColumn
            paneId={paneId}
            repo={repo}
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
