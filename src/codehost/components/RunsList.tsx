import { Segmented } from "../../ui/Controls";
import { memo, useEffect, useMemo, useRef } from "react";
import { openUrl, swallow } from "../../plugin-api/host";
import { useResourceEnabled } from "../../plugin-api/resources";
import {
    Dropdown,
    EmptyState,
    IconGit,
    IconPullRequest,
    IconRefresh,
    IconRun,
    IconTimer,
    SkeletonRows,
    StaleNotice,
    Tooltip,
} from "../../plugin-api/ui";
import { type RepoRef, type Run, type Workflow } from "../api";
import { errorText } from "../../lib/format";
import { useHost } from "../registry";
import { workflowsR } from "../resources";
import { pullListR, runListR } from "../lists";
import { elapsedMs, outcomeOf, statusParam } from "../runStatus";
import { filterBy, hostSettings, setFollowBranch, showRun, STATUS_FILTERS, updateView, type HostView, type StatusFilter } from "../state";
import { StatusMark } from "./ActionsIcon";
import { RunMenu } from "./RunMenu";
import { Face } from "./CommentThread";
import { ListTitle, ShellAccount } from "./Bits";
import { coarse } from "./hooks";
import { useNow } from "../../plugin-api/hooks";
import { useCiWatch } from "../ciWatch";
import { formatAgo, formatDuration } from "../../lib/format";
import { basename } from "../../lib/paths";

const FILTER_LABEL: Record<StatusFilter, string> = {
    all: "All",
    in_progress: "Running",
    queued: "Queued",
    success: "Passed",
    failure: "Failed",
    cancelled: "Cancelled",
};

const EVERY_WORKFLOW = "all";

/**
 * A run as GitHub lists it, with Bitbucket's face: who started it, how it went and its title, then its workflow and
 * number, commit and branch, with when it started and how long it took at the end.
 */
const RunRow = memo(function RunRow({
    paneId,
    repo,
    canWrite,
    run,
    unfinished,
    workflow,
    pullTitle,
    now,
}: {
    paneId: string;
    repo: RepoRef;
    canWrite: boolean;
    run: Run;
    unfinished: boolean;
    workflow: string | null;
    /** The title of the pull request the run belongs to, when the host has named it. */
    pullTitle: string | null;
    now: number;
}) {
    const outcome = outcomeOf(run);
    const finished = unfinished ? null : run.updatedAt;
    return (
        <div className="gha-run-item" data-outcome={outcome}>
            <button type="button" className="gha-run-row" onClick={() => showRun(paneId, run.id)}>
                <span className="gha-run-face">
                    <Face login={run.actor} url={run.avatarUrl} />
                </span>
                <span className="gha-run-top">
                    <StatusMark outcome={outcome} size={16} />
                    <span className="gha-run-name">{run.title || run.name || `Run #${run.runNumber}`}</span>
                </span>
                <span className="gha-run-sub">
                    <span className="gha-run-who">
                        <span className="gha-run-workflow">
                            {workflow ?? run.name} #{run.runNumber}
                        </span>
                        {run.shortSha ? ` · ${run.shortSha} by ${run.actor ?? "someone"}` : ` · ${run.actor ?? "someone"}`}
                    </span>
                    {run.pullRequests.length > 0 ? (
                        <span className="pr-ref gha-run-branch" title={`#${run.pullRequests.join(", #")}`}>
                            <IconPullRequest size={11} />
                            <span>{pullTitle ?? run.pullRequests.map((number) => `#${number}`).join(" ")}</span>
                        </span>
                    ) : (
                        run.branch && (
                            <span className="pr-ref gha-run-branch">
                                <IconGit size={11} />
                                <span>{run.branch}</span>
                            </span>
                        )
                    )}
                </span>
                <span className="gha-run-when">
                    <span className="gha-run-started">{formatAgo(run.startedAt ?? run.createdAt, now)}</span>
                    <span className="gha-run-took">
                        <IconTimer size={12} />
                        {formatDuration(elapsedMs(run.startedAt ?? run.createdAt, finished, now))}
                    </span>
                </span>
            </button>
            <span className="gha-run-more">
                <RunMenu run={run} unfinished={unfinished} repo={repo} canWrite={canWrite} onDeleted={() => {}} />
            </span>
        </div>
    );
});

function actionsPage(runUrl: string, workflow: Workflow | null): string {
    const actions = runUrl.replace(/\/runs\/\d+.*$/u, "");
    return workflow ? `${actions}/workflows/${basename(workflow.path)}` : actions;
}

interface Props {
    paneId: string;
    repo: RepoRef;
    project: string;
    view: HostView;
    /** The branch the list is narrowed to, if any. */
    branch: string | null;
    /** The branch checked out in the git pane, which the list can follow. */
    projectBranch: string | null;
    active: boolean;
    canWrite: boolean;
    onDispatch: (workflowId: string) => void;
}

export function RunsList({ paneId, repo, project, view, branch, projectBranch, active, canWrite, onDispatch }: Props) {
    const host = useHost();
    const followBranch = hostSettings(repo.provider).useSelect((settings) => settings.followBranch);
    const workflows = useResourceEnabled(active, workflowsR, repo);
    const chosen = (workflows.data ?? []).find((workflow) => workflow.id === view.workflowId) ?? null;
    const pulls = useResourceEnabled(active, pullListR, project, "all");
    const pullTitles = useMemo(() => new Map((pulls.data?.pulls ?? []).map(({ pull }) => [pull.number, pull.title])), [pulls.data]);
    const workflowNames = useMemo(() => new Map((workflows.data ?? []).map((workflow) => [workflow.id, workflow.name])), [workflows.data]);
    const page = useResourceEnabled(active, runListR, project, {
        workflowId: view.workflowId ?? undefined,
        status: statusParam(view.statusFilter),
        branch: branch ?? undefined,
        page: view.page,
        perPage: 30,
    });
    const runs = page.data?.runs ?? [];
    const anyRunning = runs.some((listed) => listed.unfinished);
    const now = useNow(active && anyRunning);

    useCiWatch(repo, active);

    // A different branch is a different list, so it starts from its first page.
    const shownBranch = useRef(branch);
    useEffect(() => {
        if (shownBranch.current === branch) return;
        shownBranch.current = branch;
        if (view.page !== 1) updateView(paneId, { page: 1, run: null, job: null });
    }, [branch, paneId, view.page]);

    const nextPage = page.data?.nextPage ?? null;
    const loading = page.status === "loading" && !page.data;
    const left = (
        <div className="gha-runs pr-list">
            <div className="gha-list-head">
                <ListTitle section="actions" />
                <Segmented
                    label="Which runs to list"
                    value={view.statusFilter}
                    options={STATUS_FILTERS.map((filter) => ({ value: filter, label: FILTER_LABEL[filter] }))}
                    onChange={(filter) => filterBy(paneId, { statusFilter: filter })}
                />
                {projectBranch && !view.branch && (
                    <Tooltip label={`Only show runs on ${projectBranch}`}>
                        <button
                            type="button"
                            className="gha-chip gha-follow"
                            data-on={followBranch ? "1" : "0"}
                            onClick={() => setFollowBranch(repo.provider, !followBranch)}>
                            <IconGit size={11} /> This branch
                        </button>
                    </Tooltip>
                )}
                <span className="gha-page-spacer" />
                <span className="gha-dim">
                    {page.data?.total ?? runs.length} run{(page.data?.total ?? runs.length) === 1 ? "" : "s"}
                    {branch && <span className="gha-dim"> on {branch}</span>}
                </span>
                <Dropdown
                    value={view.workflowId === null ? EVERY_WORKFLOW : view.workflowId}
                    options={[
                        { value: EVERY_WORKFLOW, label: "Every workflow" },
                        ...(workflows.data ?? []).map((workflow) => ({
                            value: workflow.id,
                            label: workflow.name,
                            detail: workflow.active ? undefined : "off",
                        })),
                    ]}
                    onChange={(value) => filterBy(paneId, { workflowId: value === EVERY_WORKFLOW ? null : value })}
                    title="Which workflow's runs to show"
                />
                {canWrite && chosen?.active && (
                    <Tooltip label="Run workflow">
                        <button type="button" className="gha-icon-btn" onClick={() => onDispatch(chosen.id)} aria-label="Run workflow">
                            <IconRun size={12} />
                        </button>
                    </Tooltip>
                )}
                <Tooltip label="Refresh">
                    <button type="button" className="gha-icon-btn" onClick={() => void page.refresh()} aria-label="Refresh runs">
                        <IconRefresh size={13} />
                    </button>
                </Tooltip>
                <ShellAccount />
            </div>
            {loading && <SkeletonRows rows={8} label="Loading runs" />}
            <StaleNotice handle={page} />
            {!loading && page.error && !page.data && (
                <EmptyState
                    title="Could not read runs"
                    message={errorText(page.error)}
                    tone="error"
                    action={{ label: "Try again", onClick: () => void page.refresh() }}
                />
            )}
            {!loading && !page.error && runs.length === 0 && (
                <EmptyState title="No runs" message={`Nothing matches this filter${branch ? ` on ${branch}` : ""}.`} />
            )}
            <div className="gha-run-rows">
                {runs.map(({ run, unfinished }) => (
                    <RunRow
                        key={run.id}
                        paneId={paneId}
                        repo={repo}
                        canWrite={canWrite}
                        run={run}
                        unfinished={unfinished}
                        workflow={chosen ? null : (workflowNames.get(run.workflowId) ?? null)}
                        pullTitle={run.pullRequests[0] ? (pullTitles.get(run.pullRequests[0]) ?? null) : null}
                        now={unfinished ? now : coarse(now)}
                    />
                ))}
            </div>
            {runs.length > 0 && (
                <div className="gha-pager">
                    {(view.page > 1 || nextPage) && (
                        <>
                            <button
                                type="button"
                                className="gha-btn"
                                disabled={view.page <= 1}
                                onClick={() => updateView(paneId, { page: view.page - 1 })}>
                                Newer
                            </button>
                            <span className="gha-dim">Page {view.page}</span>
                            <button
                                type="button"
                                className="gha-btn"
                                disabled={!nextPage}
                                onClick={() => updateView(paneId, { page: view.page + 1 })}>
                                Older
                            </button>
                        </>
                    )}
                    {runs[0] && (
                        <button
                            type="button"
                            className="gha-link"
                            onClick={() => void openUrl(actionsPage(runs[0].run.url, chosen)).catch(swallow(`open ${host.name}`))}>
                            Open on {host.name}
                        </button>
                    )}
                </div>
            )}
        </div>
    );

    return <div className="gha-full-page">{left}</div>;
}
