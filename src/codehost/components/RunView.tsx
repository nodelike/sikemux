import { memo, useCallback, useEffect, useRef, useState } from "react";
import { GitColumns } from "../../git/GitColumns";
import { FoldPanel } from "../../git/FoldPanel";
import { confirmDialog, copyText, notify, openUrl, reportError, swallow } from "../../plugin-api/host";
import { invalidate, useResourceEnabled } from "../../plugin-api/resources";
import { EmptyState, IconChevron, IconExternal, IconRefresh, SkeletonRows, Tooltip } from "../../plugin-api/ui";
import { hostApi, failureMessage, type Job, type RepoRef, type Run } from "../api";
import { useHost } from "../registry";
import { artifactsR, runAttemptR, runR, timingR } from "../resources";
import {
    elapsedMs,
    eventLabel,
    failedStep,
    formatAgo,
    formatDuration,
    isUnfinished,
    outcomeOf,
    OUTCOME_LABEL,
    summaryJobs,
    watchIsNewer,
} from "../runStatus";
import { closeRun, leaveRun, showJob, updateView, useHostView, type RunTab } from "../state";
import { OutcomeIcon } from "./ActionsIcon";
import { Annotations } from "./Annotations";
import { Approvals } from "./Approvals";
import { Artifacts } from "./Artifacts";
import { Branch, Who } from "./Bits";
import { coarse, useBusy, useNow } from "./hooks";
import { JobGraph } from "./JobGraph";
import { JobLogView } from "./JobLogView";
import { JobSummary } from "./JobSummary";
import { RunMenu } from "./RunMenu";
import { billedMinutes } from "./RunUsage";
import { WorkflowFile } from "./WorkflowFile";

const WATCH_RETRY_MS = 30_000;
const WATCH_RETRY_MAX_MS = 5 * 60_000;

const refreshRuns = () => invalidate((kind) => kind === "host.runs" || kind === "host.run");

interface OpenRun {
    run: Run | null;
    jobs: Job[];
    attempt: number | null;
    setAttempt: (attempt: number | null) => void;
    latestAttempt: number;
    /** Still going, on its latest attempt, so it is watched and its clock ticks. */
    moving: boolean;
    watchError: string | null;
    now: number;
    loading: boolean;
    error: unknown;
    refresh: () => void;
}

/**
 * One run as both columns read it: the attempt picked, and while it is going, what the backend pushes on every tick.
 * The list behind it is re-read once it ends.
 */
function useOpenRun(repo: RepoRef, runId: string, active: boolean): OpenRun {
    const [attempt, setAttempt] = useState<number | null>(null);
    const detail = useResourceEnabled(active && attempt === null, runR, repo, runId);
    const older = useResourceEnabled(active && attempt !== null, runAttemptR, repo, runId, attempt ?? 0);
    const shown = attempt === null ? detail : older;
    const [live, setLive] = useState<{ run: Run; jobs: Job[] } | null>(null);
    const [watchError, setWatchError] = useState<string | null>(null);

    const watched = attempt === null ? live : null;
    const useWatched = watchIsNewer(watched?.run ?? null, shown.data?.run ?? null);
    const run = (useWatched ? watched?.run : shown.data?.run) ?? null;
    const jobs = useWatched && watched?.jobs.length ? watched.jobs : (shown.data?.jobs ?? []);
    const moving = attempt === null && !!run && isUnfinished(run);
    const highestAttempt = useRef(0);
    const latestAttempt = Math.max(highestAttempt.current, detail.data?.run.attempt ?? 0, watched?.run.attempt ?? 0, run?.attempt ?? 0);
    highestAttempt.current = latestAttempt;
    const now = useNow(active && moving);

    const latestRun = useRef(run);
    latestRun.current = run;

    useEffect(() => {
        if (!active || !moving) return;
        let alive = true;
        let generation = 0;
        let streamId: number | null = null;
        let starting = false;
        let retry: number | undefined;
        let giveUps = 0;
        let hopeless = false;

        const stop = () => {
            generation += 1;
            window.clearTimeout(retry);
            retry = undefined;
            if (streamId !== null) void hostApi(repo.provider).watchStop(streamId).catch(swallow("stop watching the run"));
            streamId = null;
        };

        const start = () => {
            if (!alive || hopeless || document.hidden || starting || streamId !== null) return;
            const startedIn = generation;
            let ended = false;
            starting = true;
            hostApi(repo.provider)
                .watchStart(repo, runId, (tick) => {
                    if (!alive || startedIn !== generation) return;
                    if (tick.run) setLive({ run: tick.run, jobs: tick.jobs });
                    setWatchError(tick.error);
                    if (!tick.finished) {
                        giveUps = 0;
                        return;
                    }
                    ended = true;
                    streamId = null;
                    refreshRuns();
                    hopeless = tick.fatal;
                    const last = tick.run ?? latestRun.current;
                    if (hopeless || !last || !isUnfinished(last)) return;
                    retry = window.setTimeout(start, Math.min(WATCH_RETRY_MS * 2 ** giveUps, WATCH_RETRY_MAX_MS));
                    giveUps += 1;
                })
                .then((id) => {
                    if (!alive || startedIn !== generation) void hostApi(repo.provider).watchStop(id).catch(swallow("stop watching the run"));
                    else if (!ended) streamId = id;
                })
                .catch((error: unknown) => {
                    if (alive && startedIn === generation) setWatchError(failureMessage(error));
                })
                .finally(() => {
                    starting = false;
                    if (startedIn !== generation) start();
                });
        };

        const onVisibility = () => {
            if (document.hidden) stop();
            else start();
        };
        start();
        document.addEventListener("visibilitychange", onVisibility);
        return () => {
            alive = false;
            document.removeEventListener("visibilitychange", onVisibility);
            stop();
        };
    }, [active, moving, repo, runId]);

    return {
        run,
        jobs,
        attempt,
        setAttempt,
        latestAttempt,
        moving,
        watchError,
        now,
        loading: shown.status === "loading",
        error: shown.error,
        refresh: () => void shown.refresh(),
    };
}

/** The run's state and what can be done to it, where a pull request's merge box sits. */
function RunCard({
    repo,
    open,
    canWrite,
    active,
    onDeleted,
}: {
    repo: RepoRef;
    open: OpenRun;
    canWrite: boolean;
    active: boolean;
    onDeleted: () => void;
}) {
    const { ci } = useHost().capabilities;
    const run = open.run as Run;
    const outcome = outcomeOf(run);
    const live = isUnfinished(run);
    const timing = useResourceEnabled(active && !live, timingR, repo, run.id);
    const took = live ? null : (timing.data?.runDurationMs ?? null);
    const [busy, runBusy] = useBusy();
    const act = (done: string, failed: string, work: () => Promise<void>) =>
        work()
            .then(() => {
                notify("success", done);
                refreshRuns();
            })
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

    return (
        <div className="gha-merge-box">
            <div className="gha-merge-part">
                <div className="gha-merge-row">
                    <OutcomeIcon outcome={outcome} size={12} />
                    <span className="gha-merge-title">{OUTCOME_LABEL[outcome]}</span>
                    <span className="gha-merge-detail">
                        {formatDuration(took ?? elapsedMs(run.startedAt ?? run.createdAt, live ? null : run.updatedAt, open.now))}
                    </span>
                </div>
                {open.moving && open.watchError && (
                    <div className="gha-callout" data-tone="danger">
                        {open.watchError}
                    </div>
                )}
                {ci.attempts && open.latestAttempt > 1 && (
                    <div className="gha-attempts">
                        <span className="gha-dim">Attempts</span>
                        {Array.from({ length: open.latestAttempt }, (_, index) => index + 1).map((number) => (
                            <button
                                key={number}
                                type="button"
                                className="gha-chip"
                                data-on={(open.attempt ?? open.latestAttempt) === number ? "1" : "0"}
                                onClick={() => open.setAttempt(number === open.latestAttempt ? null : number)}>
                                #{number}
                            </button>
                        ))}
                    </div>
                )}
            </div>
            {ci.approvals && <Approvals repo={repo} runId={run.id} status={run.status} conclusion={run.conclusion} active={active} />}
            <div className="gha-merge-actions">
                {canWrite && live && (
                    <button type="button" className="gha-btn danger" disabled={busy} onClick={() => runBusy(cancel)}>
                        Cancel run
                    </button>
                )}
                {canWrite && ci.rerunFailed && !live && outcome !== "success" && (
                    <button
                        type="button"
                        className="gha-btn primary"
                        disabled={busy}
                        onClick={() =>
                            runBusy(() =>
                                act("Re-running the failed jobs", "Could not re-run the failed jobs", () =>
                                    hostApi(repo.provider).rerun(repo, run.id, true),
                                ),
                            )
                        }>
                        Re-run failed jobs
                    </button>
                )}
                {canWrite && !live && (
                    <button
                        type="button"
                        className="gha-btn"
                        disabled={busy}
                        onClick={() =>
                            runBusy(() =>
                                act("Re-running every job", "Could not re-run every job", () => hostApi(repo.provider).rerun(repo, run.id, false)),
                            )
                        }>
                        Re-run all jobs
                    </button>
                )}
                <span className="run-card-tools">
                    <Tooltip label="Refresh">
                        <button type="button" className="gha-icon-btn" onClick={open.refresh} aria-label="Refresh run">
                            <IconRefresh size={13} />
                        </button>
                    </Tooltip>
                    <RunMenu run={run} repo={repo} canWrite={canWrite} onDeleted={onDeleted} />
                </span>
            </div>
        </div>
    );
}

/** The run's jobs as rows, like a pull request lists its files. */
const JobRows = memo(function JobRows({ paneId, jobs, openJob, now }: { paneId: string; jobs: Job[]; openJob: string | null; now: number }) {
    return (
        <div className="git-list pr-files">
            <div className="git-group git-file-group">
                <span className="git-label">Jobs</span>
                {jobs.length > 0 && <span className="git-count">{jobs.length}</span>}
            </div>
            {jobs.length === 0 && <div className="gha-side-empty">No jobs yet</div>}
            {jobs.map((job) => {
                const outcome = outcomeOf(job);
                const stopped = failedStep(job);
                return (
                    <button
                        key={job.id}
                        type="button"
                        className={`git-row git-file-row${openJob === job.id ? " sel" : ""}`}
                        title={job.name}
                        onClick={() => showJob(paneId, job.id)}>
                        <OutcomeIcon outcome={outcome} size={12} />
                        <span className="git-row-name">{job.name}</span>
                        {stopped && <span className="gha-job-stopped">at {stopped.name}</span>}
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

/** The run's artifacts, folding at the foot of the left column. */
function ArtifactsFold({ repo, run, active }: { repo: RepoRef; run: Run; active: boolean }) {
    const { ci } = useHost().capabilities;
    const [open, setOpen] = useState(false);
    const [height, setHeight] = useState<number | null>(null);
    const finished = run.status === "completed";
    const artifacts = useResourceEnabled(active && ci.artifacts && finished, artifactsR, repo, run.id);
    const count = artifacts.data?.length ?? 0;
    if (count === 0) return null;
    return (
        <FoldPanel label="Artifacts" count={count} open={open} height={height} onToggle={() => setOpen((was) => !was)} onResize={setHeight}>
            <Artifacts repo={repo} runId={run.id} active={active} />
        </FoldPanel>
    );
}

function StepRow({ step, now, onPick }: { step: Job["steps"][number]; now: number; onPick: (number: number) => void }) {
    const outcome = outcomeOf(step);
    return (
        <button type="button" className="gha-step" data-outcome={outcome} onClick={() => onPick(step.number)} title="Show this step in the log">
            <OutcomeIcon outcome={outcome} size={11} />
            <span className="gha-step-name">{step.name}</span>
            <span className="gha-dim">{formatDuration(elapsedMs(step.startedAt, step.completedAt, now))}</span>
        </button>
    );
}

/** One job's steps, what CI flagged in it and its log, under the job's own actions. */
function JobLogs({ repo, job, now, active, canWrite }: { repo: RepoRef; job: Job; now: number; active: boolean; canWrite: boolean }) {
    const host = useHost();
    const [step, setStep] = useState<{ number: number } | null>(null);
    const [busy, runBusy] = useBusy();
    const rerun = (debug: boolean) =>
        runBusy(() =>
            hostApi(repo.provider)
                .rerunJob(repo, job.id, debug)
                .then(() => {
                    notify("success", `Re-running ${job.name}`);
                    refreshRuns();
                })
                .catch(reportError(`Could not re-run ${job.name}`)),
        );
    return (
        <div className="run-logs">
            <div className="gha-job-actions">
                <OutcomeIcon outcome={outcomeOf(job)} size={12} />
                <span className="gha-comment-author">{job.name}</span>
                {job.runner && <span className="gha-dim">on {job.runner}</span>}
                <span className="gha-page-spacer" />
                {canWrite && host.capabilities.ci.rerunJob && job.status === "completed" && (
                    <button type="button" className="gha-link" disabled={busy} onClick={() => rerun(false)}>
                        Re-run this job
                    </button>
                )}
                {canWrite && host.capabilities.ci.rerunJob && host.capabilities.ci.debugLogs && job.status === "completed" && (
                    <button
                        type="button"
                        className="gha-link"
                        disabled={busy}
                        onClick={() => rerun(true)}
                        title="Re-run with the runner's debug logging on">
                        with debug logs
                    </button>
                )}
                {job.url && (
                    <button type="button" className="gha-link" onClick={() => void openUrl(job.url ?? "").catch(swallow(`open ${host.name}`))}>
                        On {host.name}
                    </button>
                )}
            </div>
            {job.steps.length > 0 && (
                <div className="gha-steps">
                    {job.steps.map((each) => (
                        <StepRow
                            key={`${each.number}-${each.name}`}
                            step={each}
                            now={each.completedAt ? coarse(now) : now}
                            onPick={(number) => setStep({ number })}
                        />
                    ))}
                </div>
            )}
            {host.capabilities.ci.annotations && job.checkRunId !== null && <Annotations repo={repo} checkRunId={job.checkRunId} active={active} />}
            <JobLogView repo={repo} job={job} active={active} step={step} />
        </div>
    );
}

function RunSummaries({ repo, jobs, finished, active }: { repo: RepoRef; jobs: Job[]; finished: boolean; active: boolean }) {
    const [everything, setEverything] = useState(false);
    const shown = summaryJobs(jobs, finished, everything);
    const rest = summaryJobs(jobs, finished, true).length - shown.length;
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
    const live = isUnfinished(run);
    const timing = useResourceEnabled(active && ci.billing && !live, timingR, repo, run.id);
    const billable = timing.data?.billable ?? [];
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
                jobs={open.jobs}
                now={open.now}
                openJob={openJob}
                onOpen={pick}
                fileShown={showFile}
                onToggleFile={ci.workflowFile ? toggleFile : null}
            />
            {ci.workflowFile && showFile && <WorkflowFile repo={repo} workflowId={run.workflowId} active={active} />}
            {ci.summaries && <RunSummaries repo={repo} jobs={open.jobs} finished={!live} active={active} />}
        </div>
    );
}

function RunRight({
    paneId,
    repo,
    open,
    openJob,
    tab,
    active,
    canWrite,
}: {
    paneId: string;
    repo: RepoRef;
    open: OpenRun;
    openJob: string | null;
    tab: RunTab;
    active: boolean;
    canWrite: boolean;
}) {
    const host = useHost();
    const run = open.run as Run;
    const picked = open.jobs.find((job) => job.id === openJob) ?? null;
    const tabs: { id: RunTab; label: string }[] = [
        { id: "summary", label: "Summary" },
        { id: "logs", label: picked ? `Logs · ${picked.name}` : "Logs" },
    ];
    return (
        <div className="pr-right">
            <div className="git-detail">
                <div className="pr-title-row">
                    <h2 className="git-detail-title">
                        {run.title || run.name} <span className="pr-number">#{run.runNumber}</span>
                    </h2>
                    <Tooltip label={`Open on ${host.name}`}>
                        <button
                            type="button"
                            className="gha-icon-btn"
                            aria-label={`Open on ${host.name}`}
                            onClick={() => void openUrl(run.url).catch(swallow(`open ${host.name}`))}>
                            <IconExternal size={12} />
                        </button>
                    </Tooltip>
                </div>
                <div className="git-detail-meta">
                    {run.actor && <Who login={run.actor} avatarUrl={run.avatarUrl} />}
                    <span>
                        {eventLabel(run.event)} {formatAgo(run.createdAt, open.now)}
                    </span>
                    <Tooltip label="Copy the commit">
                        <button
                            type="button"
                            className="gha-link gha-mono"
                            onClick={() =>
                                void copyText(run.sha)
                                    .then(() => notify("success", `Copied ${run.shortSha}`))
                                    .catch(swallow("copy the commit"))
                            }>
                            {run.shortSha}
                        </button>
                    </Tooltip>
                    {run.branch && <Branch name={run.branch} />}
                    {run.pullRequests.map((number) => (
                        <span key={number} className="gha-item-number">
                            #{number}
                        </span>
                    ))}
                    {run.name !== run.title && <span>{run.name}</span>}
                    {run.attempt > 1 && <span>attempt {run.attempt}</span>}
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
                        repo={repo}
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

function OpenRunView({ paneId, repo, runId, openJob, active, canWrite }: Props) {
    const open = useOpenRun(repo, runId, active);
    const { runFrom, pickFailed, runTab } = useHostView(paneId);

    // A run opened from a pull request's check lands on the log of the job that failed, once its jobs are known.
    const jobsRead = open.jobs.length > 0;
    const failedJob = open.jobs.find((job) => outcomeOf(job) === "failure")?.id ?? null;
    useEffect(() => {
        if (!pickFailed || !jobsRead) return;
        updateView(paneId, openJob === null && failedJob !== null ? { pickFailed: false, job: failedJob, runTab: "logs" } : { pickFailed: false });
    }, [pickFailed, jobsRead, failedJob, openJob, paneId]);

    const back = (
        <button type="button" className="pr-back" onClick={() => leaveRun(paneId)}>
            <IconChevron size={11} /> {runFrom === null ? "Runs" : `#${runFrom}`}
        </button>
    );
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
                            message={failureMessage(open.error)}
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
                <>
                    {back}
                    <div className="pr-card">
                        <RunCard repo={repo} open={open} canWrite={canWrite} active={active} onDeleted={() => closeRun(paneId)} />
                    </div>
                    <JobRows paneId={paneId} jobs={open.jobs} openJob={openJob} now={open.now} />
                    <ArtifactsFold repo={repo} run={open.run} active={active} />
                </>
            }
            right={<RunRight paneId={paneId} repo={repo} open={open} openJob={openJob} tab={runTab} active={active} canWrite={canWrite} />}
        />
    );
}
