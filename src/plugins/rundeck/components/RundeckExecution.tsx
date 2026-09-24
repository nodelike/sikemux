import { useMemo, useState } from "react";
import { confirmDialog, openUrl, swallow } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { EmptyState, IconClock, IconGit, IconRun, IconTimer, IconUser } from "../../../plugin-api/ui";
import { errorMessage, rundeckApi, type RundeckStep } from "../api";
import * as cmd from "../state";
import type { JobRef } from "../state";
import { rndJobDetailR } from "../resources";
import { branchOf, displayStatus, duration, formatTime, isLiveStatus } from "../shape";
import { statusKind } from "./branchStyle";
import { executionProgress } from "./executionProgress";
import { useNow } from "./hooks";
import { reusableOptions } from "./options";
import { RundeckLogView } from "./RundeckLogView";
import { useExecutionStreams } from "./useExecutionStreams";

interface Props {
    paneId: string;
    level: { kind: "execution"; executionId: number } & JobRef;
    active: boolean;
}

const STEP_STATE: Record<string, { label: string; cls: "pending" | "running" | "ok" | "fail" | "skip" }> = {
    NOT_STARTED: { label: "·", cls: "pending" },
    WAITING: { label: "·", cls: "pending" },
    RUNNING: { label: "▶", cls: "running" },
    RUNNING_HANDLER: { label: "▶", cls: "running" },
    SUCCEEDED: { label: "✓", cls: "ok" },
    FAILED: { label: "✕", cls: "fail" },
    ABORTED: { label: "⊘", cls: "fail" },
    NOT_ELIGIBLE: { label: "—", cls: "skip" },
};

export function RundeckExecution({ paneId, level, active }: Props) {
    const { execution, state, terminal, watchErr, logs } = useExecutionStreams(level.executionId, active);
    const detail = useResourceEnabled(active, rndJobDetailR, level.jobId);
    const branchOptions = cmd.rundeckSettings.useSelect((s) => s.branchOptions);
    const [stepFilter, setStepFilter] = useState<string | null>(null);
    const [aborting, setAborting] = useState(false);
    const [abortErr, setAbortErr] = useState<string | null>(null);

    const rawStatus = execution?.status ?? state?.executionState ?? null;
    const status = displayStatus(rawStatus, execution?.customStatus);
    const live = isLiveStatus(rawStatus);
    const now = useNow(live);
    const options = execution?.job?.options ?? null;
    const branch = branchOf(options, branchOptions);
    const started = execution?.["date-started"]?.date ?? null;
    const ended = execution?.["date-ended"]?.date ?? null;
    const progress = executionProgress(state);
    const steps = state?.steps ?? [];
    const stepNames = detail.data?.steps ?? [];
    const stepLabel = (idx: number) => stepNames[idx] || `step ${idx + 1}`;
    const filterIndex = stepFilter ? steps.findIndex((s, i) => stepKey(s, i) === stepFilter) : -1;

    const secureNames = useMemo(() => new Set(detail.data?.options.filter((o) => o.secure).map((o) => o.name) ?? []), [detail.data]);

    const abort = async () => {
        const ok = await confirmDialog({
            title: `Abort execution #${level.executionId}?`,
            body: `${level.name} will be stopped where it is.`,
            confirmLabel: "abort",
            destructive: true,
        });
        if (!ok) return;
        setAborting(true);
        setAbortErr(null);
        try {
            const result = await rundeckApi.abort(level.executionId);
            if (result.abort?.status === "failed") setAbortErr(`Rundeck refused to abort: ${result.abort.reason ?? "no reason given"}`);
        } catch (e) {
            setAbortErr(errorMessage(e));
        } finally {
            setAborting(false);
        }
    };

    const canRunAgain = !!execution?.job;
    const runAgain = () => {
        if (!execution?.job) return;
        const job: JobRef = { project: level.project, jobId: level.jobId, name: level.name, group: level.group, repoPath: level.repoPath };
        cmd.rundeckPush(paneId, { kind: "deploy", ...job, branch: branch ?? undefined, options: reusableOptions(options, secureNames) });
    };

    return (
        <div className="rnd-exec">
            <header className="rnd-exec-head">
                <div className="rnd-exec-head-l">
                    <span className={`rnd-exec-pill rnd-status-${statusKind(rawStatus)}`}>{status}</span>
                    {live && (
                        <span className="rnd-head-progress">
                            <span
                                className={`rnd-progress-track${progress ? "" : " indeterminate"}`}
                                role="progressbar"
                                aria-label={`Execution ${level.executionId} progress`}
                                aria-valuemin={0}
                                aria-valuemax={100}
                                aria-valuenow={progress?.percent}>
                                <span className="rnd-progress-fill" style={progress ? { width: `${progress.percent}%` } : undefined} />
                            </span>
                            <span className="rnd-progress-copy">
                                {progress ? `${progress.completed} / ${progress.total} steps · ${progress.percent}%` : "syncing steps"}
                            </span>
                        </span>
                    )}
                </div>
                <div className="rnd-exec-head-r">
                    <div className="rnd-exec-meta-row">
                        {branch && (
                            <span className="rnd-exec-meta" title="branch">
                                <IconGit size={12} className="rnd-meta-ic branch" />
                                <span className="rnd-meta-v">{branch}</span>
                            </span>
                        )}
                        <span className="rnd-exec-meta dim" title="triggered by">
                            <IconUser size={12} className="rnd-meta-ic" />
                            <span className="rnd-meta-v">{execution?.user ?? "—"}</span>
                        </span>
                        <span className="rnd-exec-meta dim" title="started">
                            <IconClock size={12} className="rnd-meta-ic" />
                            <span className="rnd-meta-v">{started ? formatTime(started, true) : "—"}</span>
                        </span>
                        <span className="rnd-exec-meta dim" title="duration">
                            <IconTimer size={12} className="rnd-meta-ic" />
                            <span className="rnd-meta-v">{duration(started, ended, now) || "—"}</span>
                        </span>
                    </div>
                    <button
                        className="rnd-btn-sm rnd-btn-primary"
                        disabled={!canRunAgain}
                        onClick={runAgain}
                        title={canRunAgain ? "Open the run form with this execution's options" : "Run again unavailable for this execution"}>
                        <IconRun size={11} />
                        run again
                    </button>
                    {execution?.permalink && (
                        <button
                            type="button"
                            className="rnd-btn-sm"
                            onClick={() => void openUrl(execution.permalink!).catch(swallow("open Rundeck URL"))}>
                            open ↗
                        </button>
                    )}
                    {live && (
                        <button className="rnd-btn-sm rnd-btn-danger" disabled={aborting} onClick={() => void abort()}>
                            {aborting ? "aborting…" : "abort"}
                        </button>
                    )}
                </div>
            </header>

            {watchErr && <div className="rnd-banner warn">{watchErr}</div>}
            {abortErr && <div className="rnd-banner danger">{abortErr}</div>}

            <div className="rnd-exec-body">
                <aside className="rnd-steps">
                    <div className="rnd-steps-head">
                        <span>steps</span>
                        {stepFilter && (
                            <button className="rnd-pill-x" onClick={() => setStepFilter(null)}>
                                clear filter
                            </button>
                        )}
                    </div>
                    <div className="rnd-steps-list">
                        {steps.length === 0 && <EmptyState message="waiting for steps…" />}
                        {steps.map((s, i) => {
                            const key = stepKey(s, i);
                            return (
                                <StepRow
                                    key={key}
                                    idx={i}
                                    label={stepLabel(i)}
                                    step={s}
                                    now={now}
                                    selected={stepFilter === key}
                                    onClick={() => setStepFilter(stepFilter === key ? null : key)}
                                />
                            );
                        })}
                    </div>
                </aside>

                <RundeckLogView
                    logs={logs}
                    stepFilter={stepFilter}
                    stepLabel={filterIndex >= 0 ? stepLabel(filterIndex) : null}
                    terminal={terminal}
                    permalink={execution?.permalink ?? null}
                />
            </div>
        </div>
    );
}

function stepKey(step: RundeckStep, idx: number): string {
    return step.stepctx ?? String(idx + 1);
}

function StepRow({
    idx,
    label,
    step,
    now,
    selected,
    onClick,
}: {
    idx: number;
    label: string;
    step: RundeckStep;
    now: number;
    selected: boolean;
    onClick: () => void;
}) {
    const stateName = step.executionState ?? "NOT_STARTED";
    const ui = STEP_STATE[stateName] ?? { label: "?", cls: "pending" as const };
    return (
        <button
            className={`rnd-step${selected ? " selected" : ""} step-${ui.cls}`}
            onClick={onClick}
            title={`${label} · ${stateName.toLowerCase()}`}
            aria-pressed={selected}>
            <span className="rnd-step-num">{idx + 1}</span>
            <span className={`rnd-step-glyph step-${ui.cls}`}>{ui.label}</span>
            <span className="rnd-step-label">{label}</span>
            <span className="rnd-step-dur">{duration(step.startTime, step.endTime, now)}</span>
        </button>
    );
}
