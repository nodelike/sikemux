import { useEffect, useMemo, useRef, useState } from "react";
import { git, openUrl, swallow } from "../../../plugin-api/host";
import { errorMessage, type JobDetail, type RundeckExecution } from "../api";
import * as cmd from "../state";
import type { JobRef } from "../state";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { rndExecutionsR, rndJobDetailR, rndJobsR } from "../resources";
import { EmptyState, IconFetch, IconGit, IconRefresh, IconRun, SkeletonRows, Tooltip } from "../../../plugin-api/ui";
import { branchOf, branchOptionName, displayStatus, duration, formatTime, groupSegments, isLiveStatus } from "../shape";
import { BRANCH_GLYPH, branchKind, statusKind } from "./branchStyle";
import { executionProgress, newestExecutions } from "./executionProgress";
import { useNow } from "./hooks";
import { reusableOptions } from "./options";

interface Props {
    paneId: string;
    level: { kind: "service" } & JobRef;
    active: boolean;
}

export function RundeckService({ paneId, level, active }: Props) {
    const branchOptions = cmd.rundeckSettings.useSelect((s) => s.branchOptions);
    const detail = useResourceEnabled(active, rndJobDetailR, level.jobId);
    const jobs = useResourceEnabled(active, rndJobsR, level.project);
    const execs = useResourceEnabled(active, rndExecutionsR, level.jobId, level.project, 25);
    const [actionError, setActionError] = useState<string | null>(null);
    const [manualBranch, setManualBranch] = useState("");
    const refreshRef = useRef(execs.refresh);
    refreshRef.current = execs.refresh;

    const executions = useMemo(() => newestExecutions(execs.data ?? []), [execs.data]);
    const summary = jobs.data?.find((job) => job.id === level.jobId) ?? null;
    const anyLive = executions.some((ex) => isLiveStatus(ex.status));
    const now = useNow(active && anyLive);

    const branchKey = detail.data
        ? branchOptionName(
              detail.data.options.map((o) => o.name),
              branchOptions,
          )
        : null;
    const runsBranch = detail.data
        ? branchKey !== null
        : !!detail.error || executions.some((ex) => branchOf(ex.job?.options, branchOptions) !== null);
    const runWord = runsBranch ? "deploy" : "run";
    const executionEnabled = detail.data?.execution_enabled !== false && summary?.enabled !== false;
    const secureNames = useMemo(() => new Set(detail.data?.options.filter((o) => o.secure).map((o) => o.name) ?? []), [detail.data]);

    const lastGood = executions.find((ex) => ex.status === "succeeded" && (!runsBranch || branchOf(ex.job?.options, branchOptions) !== null)) ?? null;

    useEffect(() => {
        if (!active) return;
        const refresh = () => {
            if (!document.hidden) void refreshRef.current().catch(() => {});
        };
        const timer = window.setInterval(refresh, anyLive ? 3_000 : 20_000);
        document.addEventListener("visibilitychange", refresh);
        return () => {
            window.clearInterval(timer);
            document.removeEventListener("visibilitychange", refresh);
        };
    }, [active, anyLive, level.jobId, level.project]);

    const job: JobRef = { project: level.project, jobId: level.jobId, name: level.name, group: level.group, repoPath: level.repoPath };

    const openForm = (branch: string | undefined, options?: Record<string, string>) => {
        setActionError(null);
        cmd.rundeckPush(paneId, { kind: "deploy", ...job, branch, options });
    };

    const deployCurrentBranch = async () => {
        if (!level.repoPath) return;
        setActionError(null);
        try {
            const status = await git.status(level.repoPath);
            openForm(status.branch === "HEAD" ? "" : status.branch);
        } catch (e) {
            setActionError(errorMessage(e));
        }
    };

    const repeatLast = () => {
        if (!lastGood) return;
        openForm(branchOf(lastGood.job?.options, branchOptions) ?? undefined, reusableOptions(lastGood.job?.options, secureNames));
    };

    return (
        <div className="rnd-service">
            <ServiceHeader
                level={level}
                detail={detail.data ?? null}
                permalink={summary?.permalink ?? null}
                scheduled={summary?.scheduled ?? detail.data?.scheduled ?? false}
            />
            {detail.error && <div className="rnd-banner muted">Couldn't read job options: {detail.error}</div>}
            {!executionEnabled && <div className="rnd-banner warn">Executions are disabled for this job in Rundeck.</div>}

            <div className="rnd-svc-bar">
                {runsBranch ? (
                    <form
                        className="rnd-composer"
                        onSubmit={(e) => {
                            e.preventDefault();
                            const branch = manualBranch.trim();
                            if (branch) openForm(branch);
                        }}>
                        <input
                            type="text"
                            value={manualBranch}
                            onChange={(e) => setManualBranch(e.target.value)}
                            placeholder="branch name"
                            aria-label="Branch to deploy"
                            spellCheck={false}
                            autoCapitalize="off"
                            autoCorrect="off"
                        />
                        <button className="rnd-composer-go" disabled={!manualBranch.trim() || !executionEnabled}>
                            <IconRun size={11} />
                            deploy
                        </button>
                    </form>
                ) : (
                    <button className="rnd-btn rnd-btn-primary" onClick={() => openForm(undefined)} disabled={!executionEnabled}>
                        <IconRun size={11} />
                        run…
                    </button>
                )}

                <span className="rnd-svc-div" />

                {runsBranch && (
                    <Tooltip label={level.repoPath ? `Deploy the branch checked out in ${level.repoPath}` : "No local checkout linked to this job"}>
                        <span className="rnd-btn-wrap">
                            <button
                                className="rnd-ghost-btn"
                                onClick={() => void deployCurrentBranch()}
                                disabled={!level.repoPath || !executionEnabled}>
                                <IconGit size={13} />
                                current branch
                            </button>
                        </span>
                    </Tooltip>
                )}
                <Tooltip label={lastGood ? `Prefill from #${lastGood.id}` : `No successful ${runWord} with a branch in the list below`}>
                    <span className="rnd-btn-wrap">
                        <button className="rnd-ghost-btn" onClick={repeatLast} disabled={!lastGood || !executionEnabled}>
                            <IconFetch size={13} />
                            {runsBranch ? "redeploy last" : "rerun last"}
                        </button>
                    </span>
                </Tooltip>
                <button
                    className="rnd-icon-btn"
                    onClick={() => void execs.refresh()}
                    disabled={execs.status === "loading"}
                    aria-label="Refresh executions">
                    <IconRefresh size={13} />
                </button>
            </div>

            {actionError && <div className="rnd-banner danger">{actionError}</div>}

            <div className="rnd-history">
                <div className="rnd-history-head">
                    <span>recent executions</span>
                    <span className="rnd-history-help">click a row to open the live view</span>
                </div>
                {execs.error && <div className="rnd-banner danger">{execs.error}</div>}
                {execs.status === "loading" && !execs.data && <SkeletonRows rows={4} label="Loading executions" />}
                {executions.map((ex) => (
                    <ExecutionRow key={ex.id} paneId={paneId} job={job} ex={ex} branchOptions={branchOptions} now={now} />
                ))}
                {execs.data && execs.data.length === 0 && <EmptyState message="No executions for this job yet." />}
            </div>
        </div>
    );
}

function ServiceHeader({
    level,
    detail,
    permalink,
    scheduled,
}: {
    level: JobRef;
    detail: JobDetail | null;
    permalink: string | null;
    scheduled: boolean;
}) {
    const path = groupSegments(level.group).join(" / ");
    return (
        <div className="rnd-section-head">
            <div className="rnd-section-title">
                <span className="rnd-section-eyebrow">{path ? `${level.project} / ${path}` : level.project}</span>
                <span className="rnd-section-name">{level.name}</span>
                {detail?.description && <span className="rnd-section-desc">{detail.description}</span>}
                <span className="rnd-tags">
                    {scheduled && <span className={`rnd-tag${detail?.schedule_enabled === false ? " muted" : ""}`}>scheduled</span>}
                    {detail && !detail.execution_enabled && <span className="rnd-tag warn">disabled</span>}
                    {detail?.node_filter && (
                        <span className="rnd-tag" title={detail.node_filter}>
                            nodes: {detail.node_filter}
                        </span>
                    )}
                </span>
            </div>
            {permalink && (
                <button className="rnd-btn-sm" onClick={() => void openUrl(permalink).catch(swallow("open Rundeck URL"))}>
                    open in Rundeck ↗
                </button>
            )}
        </div>
    );
}

function ExecutionRow({
    paneId,
    job,
    ex,
    branchOptions,
    now,
}: {
    paneId: string;
    job: JobRef;
    ex: RundeckExecution;
    branchOptions: string[];
    now: number;
}) {
    const branch = branchOf(ex.job?.options, branchOptions);
    const kind = branchKind(branch);
    const started = ex["date-started"]?.date ?? null;
    const ended = ex["date-ended"]?.date ?? null;
    const live = isLiveStatus(ex.status);
    const progress = executionProgress(ex.workflowState);

    return (
        <button
            className={`rnd-exec-row${live ? " running" : ""}`}
            onClick={() => cmd.rundeckPush(paneId, { kind: "execution", ...job, executionId: ex.id })}>
            <span className={`rnd-exec-status rnd-status-${statusKind(ex.status)}`}>{displayStatus(ex.status, ex.customStatus)}</span>
            <span className="rnd-exec-id">#{ex.id}</span>
            <span className={`rnd-exec-branch rnd-branch-${kind}`}>
                {branch && <span className="rnd-cell-glyph">{BRANCH_GLYPH[kind]}</span>}
                {branch ?? "—"}
            </span>
            <span className="rnd-exec-user">{ex.user ?? "—"}</span>
            <span className="rnd-exec-when">{started ? formatTime(started) : "—"}</span>
            <span className="rnd-exec-dur">{duration(started, ended, now)}</span>
            {live && (
                <span className="rnd-row-progress">
                    <span className="rnd-progress-copy">{progress ? `${progress.completed} of ${progress.total} steps` : "syncing steps"}</span>
                    <span
                        className={`rnd-progress-track${progress ? "" : " indeterminate"}`}
                        role="progressbar"
                        aria-label={`Execution ${ex.id} progress`}
                        aria-valuemin={0}
                        aria-valuemax={100}
                        aria-valuenow={progress?.percent}>
                        <span className="rnd-progress-fill" style={progress ? { width: `${progress.percent}%` } : undefined} />
                    </span>
                    <span className="rnd-progress-value">{progress ? `${progress.percent}%` : "live"}</span>
                </span>
            )}
        </button>
    );
}
