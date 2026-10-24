import { useState } from "react";
import { confirmDialog, openUrl, swallow } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { IconCheck, IconClock, IconClose, IconExternal, IconFetch, IconGit, IconStop, IconTimer, IconUser } from "../../../plugin-api/ui";
import { errorText, formatDuration } from "../../../plugin-api/format";
import { rundeckApi } from "../api";
import * as cmd from "../state";
import type { JobRef } from "../state";
import { rndExecutionV } from "../resources";
import { useWatchingRun, watchRun } from "../runWatch";
import { duration, formatTime } from "../shape";
import type { RundeckRunStep } from "../types.generated";
import { useNow } from "../../../plugin-api/hooks";
import { RundeckLogView } from "./RundeckLogView";
import { useExecutionLog } from "./useExecutionStreams";
import { FolderChip, Header, Status, levelCrumbs } from "./parts";

interface Props {
    paneId: string;
    level: { kind: "execution"; executionId: number } & JobRef;
    active: boolean;
}

export function RundeckExecution({ paneId, level, active }: Props) {
    const page = useResourceEnabled(active, rndExecutionV, paneId, level.executionId, level.jobId, level.project);
    const logs = useExecutionLog(level.executionId, active);
    const run = page.data;
    const [stepFilter, setStepFilter] = useState<string | null>(null);
    const [aborting, setAborting] = useState(false);
    const [abortErr, setAbortErr] = useState<string | null>(null);

    const status = run?.status ?? "";
    const live = run?.live ?? false;
    const watching = useWatchingRun(level.executionId);
    const notifyWhenDone = cmd.rundeckSettings.useSelect((s) => s.notifyWhenDone);
    const now = useNow(live);
    const branch = run?.branch ?? null;
    const started = run?.startedAt ?? null;
    const ended = run?.endedAt ?? null;
    const steps = run?.steps ?? [];
    const filterIndex = stepFilter ? steps.findIndex((step) => step.key === stepFilter) : -1;

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
            setAbortErr(errorText(e));
        } finally {
            setAborting(false);
        }
    };

    const canRunAgain = run?.canRunAgain ?? false;
    const runAgain = () => {
        if (!run?.canRunAgain) return;
        const job: JobRef = { project: level.project, jobId: level.jobId, name: level.name, group: level.group, repoPath: level.repoPath };
        cmd.rundeckPush(paneId, { kind: "deploy", ...job, branch: branch ?? undefined, options: run.repeatOptions });
    };

    const runWord = run?.runWord ?? "Run";
    const usual = run?.usualSecs ?? null;
    const elapsed = started ? Math.max(0, (now - Date.parse(started)) / 1000) : null;
    const left = live && usual !== null && elapsed !== null ? usual - elapsed : null;
    const progress = run && run.total > 0 ? run : null;

    return (
        <div className="rnd-main">
            <Header
                paneId={paneId}
                crumbs={levelCrumbs(paneId, level, level.project, level.group)}
                title={`#${level.executionId}`}
                aside={
                    <span className="rnd-head-meta">
                        <FolderChip project={level.project} group={level.group} prod={run?.prod ?? false} />
                        <Status
                            status={run?.status ?? null}
                            label={
                                live && progress
                                    ? `${status[0].toUpperCase()}${status.slice(1)} · step ${Math.min(progress.total, progress.completed + 1)} of ${progress.total}`
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
                                onClick={() => watchRun(cmd.jobRefOf(level), level.executionId)}>
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
                        {run?.permalink && (
                            <button
                                type="button"
                                className="rnd-btn rnd-icon-btn"
                                aria-label="Open in Rundeck"
                                title="Open in Rundeck"
                                onClick={() => void openUrl(run.permalink!).catch(swallow("open Rundeck URL"))}>
                                <IconExternal size={13} />
                            </button>
                        )}
                    </>
                }
            />

            <div className="rnd-exec-top">
                {steps.length > 0 && (
                    <div className="rnd-progress">
                        <span className={`rnd-progress-pct ${run?.statusKind ?? "unknown"}`}>{run?.percent != null ? `${run.percent}%` : "—"}</span>
                        <div
                            className="rnd-progress-track"
                            role="progressbar"
                            aria-label={`Run ${level.executionId} progress`}
                            aria-valuemin={0}
                            aria-valuemax={100}
                            aria-valuenow={run?.percent ?? undefined}>
                            {steps.map((step) => (
                                <i key={step.key} className={step.state} />
                            ))}
                        </div>
                        <span className="rnd-progress-copy">
                            {progress ? `${progress.completed} of ${progress.total} steps` : "Syncing steps"}
                            {left !== null && (left > 0 ? ` · about ${formatDuration(left * 1000)} left` : " · taking longer than usual")}
                        </span>
                        <div className="rnd-progress-labels" aria-hidden="true">
                            {steps.map((step) => (
                                <span key={step.key} className={step.state === "running" ? "on" : undefined}>
                                    {step.name}
                                </span>
                            ))}
                        </div>
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
                        {run?.user ?? "—"}
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

            {page.error && <div className="rnd-banner warn rnd-inset">{page.error}</div>}
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
                    {steps.map((step) => (
                        <StepRow
                            key={step.key}
                            step={step}
                            now={now}
                            selected={stepFilter === step.key}
                            onClick={() => setStepFilter(stepFilter === step.key ? null : step.key)}
                        />
                    ))}
                </aside>

                <RundeckLogView
                    logs={logs}
                    stepFilter={stepFilter}
                    stepLabel={filterIndex >= 0 ? `Step ${filterIndex + 1} · ${steps[filterIndex].name}` : null}
                    terminal={!!run && !run.live}
                    permalink={run?.permalink ?? null}
                />
            </div>
        </div>
    );
}

function StepRow({ step, now, selected, onClick }: { step: RundeckRunStep; now: number; selected: boolean; onClick: () => void }) {
    return (
        <button className={`rnd-step${selected ? " on" : ""}`} onClick={onClick} title={`${step.name} · ${step.stateName}`} aria-pressed={selected}>
            <span className={`rnd-step-icon ${step.state}`}>
                {step.state === "ok" && <IconCheck size={9} />}
                {step.state === "fail" && <IconClose size={9} />}
            </span>
            <span className="rnd-step-label">{step.name}</span>
            <span className="rnd-step-dur">{duration(step.startedAt, step.endedAt, now)}</span>
        </button>
    );
}
