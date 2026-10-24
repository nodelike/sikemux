import { Segmented } from "../../ui/Controls";
import { memo, useCallback, useEffect, useState } from "react";
import { GitColumns } from "../../git/GitColumns";
import { SwipePages } from "../../ui/SwipePages";
import { confirmDialog, copyText, notify, openUrl, reportError, swallow } from "../../plugin-api/host";
import { useResourceEnabled } from "../../plugin-api/resources";
import {
    EmptyState,
    IconChevron,
    IconClose,
    IconDownload,
    IconExternal,
    IconRefresh,
    IconWorkflow,
    SkeletonRows,
    Tooltip,
} from "../../plugin-api/ui";
import { hostApi, type Job, type ListedJob, type OpenRun as RunRead, type RepoRef, type Run } from "../api";
import { errorText } from "../../lib/format";
import { useHost } from "../registry";
import { artifactsR } from "../resources";
import { runViewR } from "../lists";
import { elapsedMs, eventLabel, outcomeOf, OUTCOME_LABEL, SUMMARY_LIMIT } from "../runStatus";
import { closeRun, leaveRun, showJob, updateView, useHostView, type RunTab } from "../state";
import { StatusMark } from "./ActionsIcon";
import { Annotations } from "./Annotations";
import { Approvals } from "./Approvals";
import { Artifacts } from "./Artifacts";
import { Face } from "./CommentThread";
import { ShellAccount } from "./Bits";
import { coarse, useBusy } from "./hooks";
import { useNow } from "../../plugin-api/hooks";
import { JobGraph } from "./JobGraph";
import { JobLogView } from "./JobLogView";
import { JobSummary } from "./JobSummary";
import { RunMenu } from "./RunMenu";
import { billedMinutes } from "./RunUsage";
import { WorkflowFile } from "./WorkflowFile";
import { formatAgo, formatDuration } from "../../lib/format";

interface OpenRun {
    read: RunRead | null;
    run: Run | null;
    jobs: ListedJob[];
    attempt: number | null;
    setAttempt: (attempt: number | null) => void;
    latestAttempt: number;
    /** Still going, on its latest attempt, so its clock ticks. */
    moving: boolean;
    now: number;
    loading: boolean;
    error: unknown;
    refresh: () => void;
}

const NO_JOBS: ListedJob[] = [];

/** One run as both columns read it: the attempt picked, which the core reads again while it is going. */
function useOpenRun(project: string | null, repo: RepoRef, runId: string, active: boolean): OpenRun {
    const [attempt, setAttempt] = useState<number | null>(null);
    const shown = useResourceEnabled(active, runViewR, project, repo, runId, attempt);
    const read = shown.data ?? null;
    const moving = attempt === null && !!read?.unfinished;
    const now = useNow(active && moving);
    return {
        read,
        run: read?.run ?? null,
        jobs: read?.jobs ?? NO_JOBS,
        attempt,
        setAttempt,
        latestAttempt: read?.attempts ?? 0,
        moving,
        now,
        loading: shown.status === "loading",
        error: shown.error,
        refresh: () => void shown.refresh(),
    };
}

/** The run's state and what can be done to it, where a pull request's merge box sits. */
/**
 * The head of an open run's side panel, like the git pane's: how it went, its number and branch with a way back, then
 * its actions edge to edge, and what still waits on someone under them.
 */
function RunHead({
    repo,
    open,
    canWrite,
    back,
    onBack,
    onDeleted,
}: {
    repo: RepoRef;
    open: OpenRun;
    canWrite: boolean;
    back: string;
    onBack: () => void;
    onDeleted: () => void;
}) {
    const host = useHost();
    const { ci } = host.capabilities;
    const run = open.run as Run;
    const outcome = outcomeOf(run);
    const live = !!open.read?.unfinished;
    const took = open.read?.timing?.runDurationMs ?? null;
    const [busy, runBusy] = useBusy();
    const act = (done: string, failed: string, work: () => Promise<void>) =>
        work()
            .then(() => notify("success", done))
            .catch(reportError(failed));

    const cancel = async () => {
        const sure = await confirmDialog({
            title: "Cancel this run?",
            body: `Run #${run.runNumber} will stop where it is.`,
            confirmLabel: "Cancel run",
            destructive: true,
        });
        if (sure) await act("Cancelled the run", "Could not cancel the run", () => hostApi(repo.provider).cancel(repo, run.id));
    };
    const rerun = (failedOnly: boolean) =>
        runBusy(() =>
            act(
                failedOnly ? "Re-running the failed jobs" : "Re-running every job",
                failedOnly ? "Could not re-run the failed jobs" : "Could not re-run every job",
                () => hostApi(repo.provider).rerun(repo, run.id, failedOnly),
            ),
        );
    const failed = !live && outcome !== "success";

    return (
        <div className="git-side-head">
            <div className="git-side-title">
                <Tooltip label={`Back to ${back}`}>
                    <button type="button" className="git-foot-btn icon pr-back-btn" aria-label={`Back to ${back}`} onClick={onBack}>
                        <IconChevron size={11} />
                    </button>
                </Tooltip>
                <StatusMark outcome={outcome} size={15} />
                <span className="pr-side-number">#{run.runNumber}</span>
                {run.branch && <span className="pr-ref">{run.branch}</span>}
                <span className="run-side-took" title={OUTCOME_LABEL[outcome]}>
                    {formatDuration(took ?? elapsedMs(run.startedAt ?? run.createdAt, live ? null : run.updatedAt, open.now))}
                </span>
                <Tooltip label="Refresh">
                    <button type="button" className="git-foot-btn icon" onClick={open.refresh} aria-label="Refresh run">
                        <IconRefresh size={13} />
                    </button>
                </Tooltip>
                <RunMenu run={run} unfinished={live} repo={repo} canWrite={canWrite} onDeleted={onDeleted} />
                <ShellAccount />
            </div>
            <div className="git-btn-group git-side-actions run-side-actions" role="group" aria-label="Run">
                {canWrite && live && (
                    <button type="button" className="git-btn danger" disabled={busy} onClick={() => runBusy(cancel)}>
                        <IconClose size={13} />
                        Cancel run
                    </button>
                )}
                {canWrite && !live && ci.rerunFailed && failed && (
                    <button type="button" className="git-btn primary" disabled={busy} onClick={() => rerun(true)}>
                        <IconRefresh size={13} />
                        Re-run failed
                    </button>
                )}
                {canWrite && !live && (
                    <button type="button" className="git-btn" disabled={busy} onClick={() => rerun(false)}>
                        <IconRefresh size={13} />
                        Re-run all
                    </button>
                )}
                <button type="button" className="git-btn" onClick={() => void openUrl(run.url).catch(swallow(`open ${host.name}`))}>
                    <IconExternal size={13} />
                    {host.name}
                </button>
            </div>
            {ci.attempts && open.latestAttempt > 1 && (
                <Segmented
                    fill
                    small
                    label="Attempts"
                    value={open.attempt ?? open.latestAttempt}
                    options={Array.from({ length: open.latestAttempt }, (_, index) => ({ value: index + 1, label: `Attempt ${index + 1}` }))}
                    onChange={(number) => open.setAttempt(number === open.latestAttempt ? null : number)}
                />
            )}
            {ci.approvals && <Approvals repo={repo} runId={run.id} pending={open.read?.approvals ?? []} />}
        </div>
    );
}

/** The run's jobs as rows, like a pull request lists its files. */
const JobRows = memo(function JobRows({ paneId, jobs, openJob, now }: { paneId: string; jobs: ListedJob[]; openJob: string | null; now: number }) {
    const [folded, setFolded] = useState(false);
    return (
        <div className="git-list pr-files">
            <div className="git-group git-file-group">
                <button type="button" className="git-group-toggle" aria-expanded={!folded} onClick={() => setFolded((was) => !was)}>
                    <IconChevron size={13} className="git-group-chev" />
                    <span className="git-group-icon">
                        <IconWorkflow size={13} />
                    </span>
                    <span className="git-group-name">Jobs</span>
                </button>
                {jobs.length > 0 && <span className="git-group-count">{jobs.length}</span>}
            </div>
            {jobs.length === 0 && <div className="gha-side-empty">No jobs yet</div>}
            {!folded &&
                jobs.map(({ job, stoppedAt }, index) => {
                    const outcome = outcomeOf(job);
                    return (
                        <button
                            key={job.id}
                            type="button"
                            className={`git-row git-tree-row git-file-row run-job-row${openJob === job.id ? " sel" : ""}`}
                            data-first={index === 0 ? "" : undefined}
                            data-last={index === jobs.length - 1 ? "" : undefined}
                            title={job.name}
                            onClick={() => showJob(paneId, job.id)}>
                            <StatusMark outcome={outcome} size={14} />
                            <span className="git-row-name">{job.name}</span>
                            {stoppedAt && <span className="gha-job-stopped">at {stoppedAt}</span>}
                            <span className="run-job-took">
                                {outcome === "skipped"
                                    ? "skipped"
                                    : formatDuration(elapsedMs(job.startedAt, job.completedAt, job.completedAt ? coarse(now) : now))}
                            </span>
                        </button>
                    );
                })}
        </div>
    );
});

/** What CI flagged in one job and its log step by step, under the job's own actions. */
function JobLogs({
    project,
    repo,
    runId,
    job,
    now,
    active,
    canWrite,
}: {
    project: string | null;
    repo: RepoRef;
    runId: string;
    job: Job;
    now: number;
    active: boolean;
    canWrite: boolean;
}) {
    const host = useHost();
    const [busy, runBusy] = useBusy();
    const rerun = (debug: boolean) =>
        runBusy(() =>
            hostApi(repo.provider)
                .rerunJob(repo, job.id, debug)
                .then(() => notify("success", `Re-running ${job.name}`))
                .catch(reportError(`Could not re-run ${job.name}`)),
        );
    return (
        <div className="run-logs">
            <div className="gha-job-actions">
                <StatusMark outcome={outcomeOf(job)} size={14} />
                <span className="gha-job-name">{job.name}</span>
                {job.runner && <span className="gha-dim gha-job-runner">on {job.runner}</span>}
                <span className="gha-page-spacer" />
                {canWrite && host.capabilities.ci.rerunJob && job.status === "completed" && (
                    <button type="button" className="git-foot-btn" disabled={busy} onClick={() => rerun(false)}>
                        <IconRefresh size={13} />
                        Re-run this job
                    </button>
                )}
                {canWrite && host.capabilities.ci.rerunJob && host.capabilities.ci.debugLogs && job.status === "completed" && (
                    <button
                        type="button"
                        className="git-foot-btn"
                        disabled={busy}
                        onClick={() => rerun(true)}
                        title="Re-run with the runner's debug logging on">
                        With debug logs
                    </button>
                )}
                {job.url && (
                    <button type="button" className="git-foot-btn" onClick={() => void openUrl(job.url ?? "").catch(swallow(`open ${host.name}`))}>
                        <IconExternal size={13} />
                        {host.name}
                    </button>
                )}
            </div>
            {host.capabilities.ci.annotations && job.checkRunId !== null && <Annotations repo={repo} checkRunId={job.checkRunId} active={active} />}
            <JobLogView project={project} repo={repo} runId={runId} job={job} now={now} active={active} />
        </div>
    );
}

/** Each summary is its own call, so a wide matrix only loads the first few until asked for the rest. */
function RunSummaries({ repo, open, active }: { repo: RepoRef; open: OpenRun; active: boolean }) {
    const [everything, setEverything] = useState(false);
    const ids = open.read?.summaries ?? [];
    const all = ids.flatMap((id) => open.jobs.find(({ job }) => job.id === id)?.job ?? []);
    const shown = everything ? all : all.slice(0, SUMMARY_LIMIT);
    const rest = all.length - shown.length;
    if (shown.length === 0) return null;
    return (
        <div className="gha-run-summaries">
            {shown.map((job) => (
                <JobSummary key={job.id} repo={repo} checkRunId={job.checkRunId ?? ""} active={active} jobName={job.name} />
            ))}
            {rest > 0 && (
                <button type="button" className="gha-link" onClick={() => setEverything(true)}>
                    Look for summaries from {rest} more job{rest === 1 ? "" : "s"}
                </button>
            )}
        </div>
    );
}

/** The run at a glance: the graph that picks a job, the workflow file, and what each job wrote as its summary. */
function RunSummary({
    paneId,
    repo,
    open,
    openJob,
    active,
}: {
    paneId: string;
    repo: RepoRef;
    open: OpenRun;
    openJob: string | null;
    active: boolean;
}) {
    const { ci } = useHost().capabilities;
    const run = open.run as Run;
    const [showFile, setShowFile] = useState(false);
    const billable = ci.billing ? (open.read?.timing?.billable ?? []) : [];
    const minutes = billedMinutes(billable);
    const pick = useCallback((jobId: string) => showJob(paneId, jobId), [paneId]);
    const toggleFile = useCallback(() => setShowFile((was) => !was), []);
    return (
        <div className="run-summary">
            {minutes > 0 && (
                <div
                    className="gha-dim run-billed"
                    title={billable.map((each) => `${each.runner}: ${formatDuration(each.totalMs)} over ${each.jobs} jobs`).join("\n")}>
                    Billed {minutes} min
                </div>
            )}
            <JobGraph
                run={run}
                read={open.read as RunRead}
                now={open.now}
                openJob={openJob}
                onOpen={pick}
                fileShown={showFile}
                onToggleFile={ci.workflowFile ? toggleFile : null}
            />
            {ci.workflowFile && showFile && <WorkflowFile repo={repo} workflowId={run.workflowId} active={active} />}
            {ci.summaries && <RunSummaries repo={repo} open={open} active={active} />}
        </div>
    );
}

function RunRight({
    paneId,
    project,
    repo,
    open,
    openJob,
    tab,
    active,
    canWrite,
}: {
    paneId: string;
    project: string | null;
    repo: RepoRef;
    open: OpenRun;
    openJob: string | null;
    tab: RunTab;
    active: boolean;
    canWrite: boolean;
}) {
    const host = useHost();
    const run = open.run as Run;
    const picked = open.jobs.find(({ job }) => job.id === openJob)?.job ?? null;
    const tabs: { id: RunTab; label: string }[] = [
        { id: "summary", label: "Summary" },
        { id: "logs", label: picked ? `Logs · ${picked.name}` : "Logs" },
    ];
    return (
        <div className="pr-right">
            <div className="git-detail">
                <div className="pr-headline">
                    <span className="pr-face">
                        <Face login={run.actor} url={run.avatarUrl} />
                    </span>
                    <span className="pr-headline-top">
                        <StatusMark outcome={outcomeOf(run)} size={16} />
                        <h2 className="git-detail-title pr-headline-title">{run.title || run.name}</h2>
                        <Tooltip label={`Open on ${host.name}`}>
                            <button
                                type="button"
                                className="gha-icon-btn"
                                aria-label={`Open on ${host.name}`}
                                onClick={() => void openUrl(run.url).catch(swallow(`open ${host.name}`))}>
                                <IconExternal size={12} />
                            </button>
                        </Tooltip>
                    </span>
                    <span className="pr-headline-sub">
                        <span className="pr-headline-who">
                            {run.name} #{run.runNumber} · {run.actor ?? "someone"} · {eventLabel(run.event).toLowerCase()}{" "}
                            {formatAgo(run.createdAt, open.now)}
                            {run.attempt > 1 ? ` · attempt ${run.attempt}` : ""}
                        </span>
                        <Tooltip label="Copy the commit">
                            <button
                                type="button"
                                className="pr-ref gha-mono run-head-sha"
                                onClick={() =>
                                    void copyText(run.sha)
                                        .then(() => notify("success", `Copied ${run.shortSha}`))
                                        .catch(swallow("copy the commit"))
                                }>
                                {run.shortSha}
                            </button>
                        </Tooltip>
                        {run.branch && <span className="pr-ref pr-ref-head">{run.branch}</span>}
                        {run.pullRequests.map((number) => (
                            <span key={number} className="pr-ref">
                                #{number}
                            </span>
                        ))}
                    </span>
                </div>
                <div className="git-detail-actions" role="tablist" aria-label="Run">
                    {tabs.map((each) => (
                        <button
                            key={each.id}
                            type="button"
                            role="tab"
                            aria-selected={tab === each.id}
                            className="gha-chip pr-tab"
                            data-on={tab === each.id ? "1" : "0"}
                            onClick={() => updateView(paneId, { runTab: each.id })}>
                            {each.label}
                        </button>
                    ))}
                </div>
            </div>
            <div className="pr-conversation">
                {tab === "summary" ? (
                    <RunSummary paneId={paneId} repo={repo} open={open} openJob={openJob} active={active} />
                ) : picked ? (
                    <JobLogs
                        key={picked.id}
                        project={project}
                        repo={repo}
                        runId={run.id}
                        job={picked}
                        now={picked.completedAt ? coarse(open.now) : open.now}
                        active={active}
                        canWrite={canWrite}
                    />
                ) : (
                    <EmptyState message="Pick a job on the left to read its log." />
                )}
            </div>
        </div>
    );
}

interface Props {
    paneId: string;
    /** The project whose code host the run is on, or null when the repository is named outright. */
    project: string | null;
    repo: RepoRef;
    runId: string;
    openJob: string | null;
    active: boolean;
    canWrite: boolean;
}

/** An open run in the git pane's columns: its card, jobs and artifacts on the left; its summary or a job's log on the right. */
export function RunView(props: Props) {
    return <OpenRunView key={props.runId} {...props} />;
}

function OpenRunView({ paneId, project, repo, runId, openJob, active, canWrite }: Props) {
    const open = useOpenRun(project, repo, runId, active);
    const { runFrom, pickFailed, runTab } = useHostView(paneId);

    // A run opened from a pull request's check lands on the log of the job that failed, once its jobs are known.
    const jobsRead = open.jobs.length > 0;
    const failedJob = open.read?.failedJob ?? null;
    useEffect(() => {
        if (!pickFailed || !jobsRead) return;
        updateView(paneId, openJob === null && failedJob !== null ? { pickFailed: false, job: failedJob, runTab: "logs" } : { pickFailed: false });
    }, [pickFailed, jobsRead, failedJob, openJob, paneId]);

    const backTo = runFrom === null ? "the runs" : `#${runFrom}`;
    const back = (
        <button type="button" className="pr-back" onClick={() => leaveRun(paneId)}>
            <IconChevron size={11} /> {runFrom === null ? "Runs" : `#${runFrom}`}
        </button>
    );
    const { ci } = useHost().capabilities;
    const [page, setPage] = useState("jobs");
    const finished = open.run?.status === "completed";
    const artifacts = useResourceEnabled(active && ci.artifacts && finished, artifactsR, repo, open.run?.id ?? "");
    const artifactCount = artifacts.data?.length ?? 0;
    if (open.loading && !open.run) return <GitColumns paneId={paneId} left={back} right={<SkeletonRows rows={8} label="Loading run" />} />;
    if (!open.run) {
        return (
            <GitColumns
                paneId={paneId}
                left={back}
                right={
                    open.error ? (
                        <EmptyState
                            title="Could not read the run"
                            message={errorText(open.error)}
                            tone="error"
                            action={{ label: "Try again", onClick: open.refresh }}
                        />
                    ) : (
                        <EmptyState message="That run is gone." />
                    )
                }
            />
        );
    }
    return (
        <GitColumns
            paneId={paneId}
            left={
                <div className="git-side">
                    <RunHead
                        repo={repo}
                        open={open}
                        canWrite={canWrite}
                        back={backTo}
                        onBack={() => leaveRun(paneId)}
                        onDeleted={() => closeRun(paneId)}
                    />
                    <SwipePages
                        label="Run"
                        current={artifactCount > 0 ? page : "jobs"}
                        onChange={setPage}
                        pages={[
                            {
                                id: "jobs",
                                label: "Jobs",
                                icon: <IconWorkflow size={14} />,
                                content: <JobRows paneId={paneId} jobs={open.jobs} openJob={openJob} now={open.now} />,
                            },
                            ...(artifactCount > 0
                                ? [
                                      {
                                          id: "artifacts",
                                          label: `Artifacts (${artifactCount})`,
                                          icon: <IconDownload size={14} />,
                                          content: (
                                              <div className="git-list pr-files">
                                                  <Artifacts repo={repo} runId={open.run.id} active={active} />
                                              </div>
                                          ),
                                      },
                                  ]
                                : []),
                        ]}
                    />
                </div>
            }
            right={
                <div className="git-right-review">
                    <RunRight
                        paneId={paneId}
                        project={project}
                        repo={repo}
                        open={open}
                        openJob={openJob}
                        tab={runTab}
                        active={active}
                        canWrite={canWrite}
                    />
                </div>
            }
        />
    );
}
