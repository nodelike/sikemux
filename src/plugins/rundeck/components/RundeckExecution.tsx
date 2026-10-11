import { useMemo, useState } from "react";
import { confirmDialog, openUrl, swallow } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { IconCheck, IconClock, IconClose, IconExternal, IconFetch, IconGit, IconStop, IconTimer, IconUser } from "../../../plugin-api/ui";
import { errorMessage, rundeckApi, type RundeckStep } from "../api";
import * as cmd from "../state";
import type { JobRef } from "../state";
import { rndJobDetailR } from "../resources";
import { useWatchingRun, watchRun } from "../runWatch";
import { branchOf, displayStatus, duration, formatTime, isLiveStatus } from "../shape";
import { executionProgress } from "./executionProgress";
import { useNow } from "./hooks";
import { reusableOptions } from "./options";
import { RundeckLogView } from "./RundeckLogView";
import { useExecutionStreams } from "./useExecutionStreams";
import { FolderChip, Header, Status, levelCrumbs } from "./parts";

interface Props {
    paneId: string;
    level: { kind: "execution"; executionId: number } & JobRef;
    active: boolean;
}

const STEP_STATE: Record<string, { cls: "pending" | "running" | "ok" | "fail" | "skip" }> = {
    NOT_STARTED: { cls: "pending" },
    WAITING: { cls: "pending" },
    RUNNING: { cls: "running" },
    RUNNING_HANDLER: { cls: "running" },
    SUCCEEDED: { cls: "ok" },
    FAILED: { cls: "fail" },
    ABORTED: { cls: "fail" },
    NOT_ELIGIBLE: { cls: "skip" },
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
    const watching = useWatchingRun(level.executionId);
    const notifyWhenDone = cmd.rundeckSettings.useSelect((s) => s.notifyWhenDone);
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

    const runWord = branch ? "Deploy" : "Run";
    const doneSteps = progress?.completed ?? 0;

    return (
        <div className="rnd-main">
            <Header
                paneId={paneId}
                crumbs={levelCrumbs(paneId, level, level.project, level.group)}
                title={`#${level.executionId}`}
                aside={
                    <span className="rnd-head-meta">
                        <FolderChip project={level.project} group={level.group} />
                        <Status
                            status={rawStatus}
                            label={
                                live && progress
                                    ? `${status[0].toUpperCase()}${status.slice(1)} · step ${Math.min(progress.total, doneSteps + 1)} of ${progress.total}`
                                    : undefined
                            }
                        />
                    </span>
                }
                tools={
                    <>
                        <button
                            className="rnd-btn"
                            disabled={!canRunAgain}
                            onClick={runAgain}
                            title={canRunAgain ? "Open the run form with this run's options" : "This run can't be repeated from here"}>
                            <IconFetch size={13} />
                            {runWord} again
                        </button>
                        {live && notifyWhenDone && (
                            <button
                                className="rnd-btn"
                                disabled={watching}
                                title={watching ? "Sikemux will tell you when this run ends" : "Get a notification when this run ends"}
                                onClick={() => void watchRun(cmd.jobRefOf(level), level.executionId).catch(swallow("watch the run"))}>
                                <IconClock size={12} />
                                {watching ? "Will notify" : "Notify me"}
                            </button>
                        )}
                        {live && (
                            <button className="rnd-btn danger" disabled={aborting} onClick={() => void abort()}>
                                <IconStop size={11} />
                                {aborting ? "Aborting…" : "Abort"}
                            </button>
                        )}
                        {execution?.permalink && (
                            <button
                                type="button"
                                className="rnd-btn rnd-icon-btn"
                                aria-label="Open in Rundeck"
                                title="Open in Rundeck"
                                onClick={() => void openUrl(execution.permalink!).catch(swallow("open Rundeck URL"))}>
                                <IconExternal size={13} />
                            </button>
                        )}
                    </>
                }
            />

            <div className="rnd-exec-top">
                {steps.length > 0 && (
                    <div className="rnd-progress">
                        <div
                            className="rnd-progress-track"
                            role="progressbar"
                            aria-label={`Run ${level.executionId} progress`}
                            aria-valuemin={0}
                            aria-valuemax={100}
                            aria-valuenow={progress?.percent}>
                            {steps.map((step, i) => (
                                <i key={stepKey(step, i)} className={stepUi(step).cls} />
                            ))}
                        </div>
                        <span className="rnd-progress-copy">{progress ? `${progress.completed} of ${progress.total} steps` : "Syncing steps"}</span>
                    </div>
                )}
                <div className="rnd-metas">
                    {branch && (
                        <span className="rnd-meta" title="Branch">
                            <IconGit size={12} />
                            {branch}
                        </span>
                    )}
                    <span className="rnd-meta" title="Started by">
                        <IconUser size={12} />
                        {execution?.user ?? "—"}
                    </span>
                    <span className="rnd-meta" title="Started">
                        <IconClock size={12} />
                        {started ? formatTime(started, true) : "—"}
                    </span>
                    <span className="rnd-meta" title="Duration">
                        <IconTimer size={12} />
                        {duration(started, ended, now) || "—"}
                    </span>
                </div>
            </div>

            {watchErr && <div className="rnd-banner warn rnd-inset">{watchErr}</div>}
            {abortErr && <div className="rnd-banner danger rnd-inset">{abortErr}</div>}

            <div className="rnd-exec-body">
                <aside className="rnd-steps" aria-label="Steps">
                    <div className="rnd-steps-head">Steps</div>
                    <button
                        className={`rnd-step${stepFilter === null ? " on" : ""}`}
                        onClick={() => setStepFilter(null)}
                        aria-pressed={stepFilter === null}>
                        <span className="rnd-step-icon all" />
                        <span className="rnd-step-label">All output</span>
                        <span className="rnd-step-dur">{logs.rows.length}</span>
                    </button>
                    {steps.length === 0 && <div className="rnd-insp-note">Waiting for steps…</div>}
                    {steps.map((s, i) => {
                        const key = stepKey(s, i);
                        return (
                            <StepRow
                                key={key}
                                label={stepLabel(i)}
                                step={s}
                                now={now}
                                selected={stepFilter === key}
                                onClick={() => setStepFilter(stepFilter === key ? null : key)}
                            />
                        );
                    })}
                </aside>

                <RundeckLogView
                    logs={logs}
                    stepFilter={stepFilter}
                    stepLabel={filterIndex >= 0 ? `Step ${filterIndex + 1} · ${stepLabel(filterIndex)}` : null}
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

function stepUi(step: RundeckStep) {
    const stateName = step.executionState ?? "NOT_STARTED";
    return { name: stateName, ...(STEP_STATE[stateName] ?? { cls: "pending" as const }) };
}

function StepRow({ label, step, now, selected, onClick }: { label: string; step: RundeckStep; now: number; selected: boolean; onClick: () => void }) {
    const ui = stepUi(step);
    return (
        <button
            className={`rnd-step${selected ? " on" : ""}`}
            onClick={onClick}
            title={`${label} · ${ui.name.toLowerCase().replace(/_/g, " ")}`}
            aria-pressed={selected}>
            <span className={`rnd-step-icon ${ui.cls}`}>
                {ui.cls === "ok" && <IconCheck size={9} />}
                {ui.cls === "fail" && <IconClose size={9} />}
            </span>
            <span className="rnd-step-label">{label}</span>
            <span className="rnd-step-dur">{duration(step.startTime, step.endTime, now)}</span>
        </button>
    );
}
