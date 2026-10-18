import { useMemo, useState } from "react";
import { currentBranch, git, openUrl, swallow } from "../../../plugin-api/host";
import { useEvery, useNow } from "../../../plugin-api/hooks";
import { formatAgo, formatDuration } from "../../../plugin-api/format";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { EmptyState, IconExternal, IconFetch, IconGit, IconRun, IconWarning, SkeletonRows, Tooltip } from "../../../plugin-api/ui";
import { failureMessage, type RundeckExecution } from "../api";
import { rndExecutionsR, rndJobDetailR, rndJobsR, useBranchOptions } from "../resources";
import * as cmd from "../state";
import type { JobRef } from "../state";
import { branchOf, branchOptionName, isProdTarget } from "../shape";
import { executionProgress, newestExecutions } from "./executionProgress";
import { reusableOptions } from "./options";
import { FolderChip, Header, Tag, levelCrumbs } from "./parts";
import { DeployTimes, RunHistory } from "./RunHistory";
import { endedAt, fastestAndSlowest, liveRun, runSeconds, usualSeconds } from "./runStats";

interface Props {
    paneId: string;
    active: boolean;
    level: { kind: "service" } & JobRef;
}

const HISTORY = 30;

export function RundeckService({ paneId, active, level }: Props) {
    const job = cmd.jobRefOf(level);
    const branchOptions = useBranchOptions();
    const isProd = cmd.rundeckSettings.useSelect((s) => isProdTarget(job.project, job.group, s.prodEnvs));
    const detail = useResourceEnabled(active, rndJobDetailR, job.jobId);
    const jobs = useResourceEnabled(active, rndJobsR, job.project);
    const execs = useResourceEnabled(active, rndExecutionsR, job.jobId, job.project, HISTORY);
    const [actionError, setActionError] = useState<string | null>(null);

    const runs = useMemo(() => newestExecutions(execs.data ?? []), [execs.data]);
    const running = liveRun(runs);
    const now = useNow(active && !!running);
    useEvery(active, running ? 3_000 : 20_000, () => void execs.refresh().catch(() => {}));

    const branchFor = (ex: RundeckExecution) => branchOf(ex.job?.options, branchOptions);
    const summary = jobs.data?.find((j) => j.id === job.jobId) ?? null;
    const runsBranch = detail.data
        ? branchOptionName(
              detail.data.options.map((o) => o.name),
              branchOptions,
          ) !== null
        : runs.some((ex) => branchFor(ex) !== null);
    const runWord = runsBranch ? "Deploy" : "Run";
    const executionEnabled = detail.data?.execution_enabled !== false;
    const secureNames = useMemo(() => new Set(detail.data?.options.filter((o) => o.secure).map((o) => o.name) ?? []), [detail.data]);
    const lastGood = runs.find((ex) => ex.status === "succeeded" && (!runsBranch || branchFor(ex) !== null)) ?? null;
    const lastGoodBranch = lastGood ? branchFor(lastGood) : null;
    const finished = runs.filter((ex) => ex.status !== "running");
    const succeeded = finished.filter((ex) => ex.status === "succeeded").length;
    const usual = usualSeconds(runs, now);
    const range = fastestAndSlowest(runs, now);

    const openForm = (branch: string | undefined, options?: Record<string, string>) => {
        setActionError(null);
        cmd.rundeckPush(paneId, { kind: "deploy", ...job, branch, options });
    };
    const deployCheckout = async () => {
        if (!job.repoPath) return;
        setActionError(null);
        try {
            openForm(currentBranch(await git.status(job.repoPath)) ?? "");
        } catch (e) {
            setActionError(failureMessage(e));
        }
    };
    const openRun = (ex: RundeckExecution) => cmd.rundeckPush(paneId, { kind: "execution", ...job, executionId: ex.id });

    return (
        <div className="rnd-main">
            <Header
                paneId={paneId}
                crumbs={levelCrumbs(paneId, level, job.project, job.group)}
                title={job.name}
                aside={<FolderChip project={job.project} group={job.group} />}
                tools={
                    <>
                        {summary?.permalink && (
                            <Tooltip label="Open in Rundeck">
                                <button
                                    className="rnd-btn rnd-icon-btn"
                                    onClick={() => void openUrl(summary.permalink!).catch(swallow("open Rundeck URL"))}
                                    aria-label="Open in Rundeck">
                                    <IconExternal size={13} />
                                </button>
                            </Tooltip>
                        )}
                        {runsBranch && job.repoPath && (
                            <Tooltip label={`Deploy the branch checked out in ${job.repoPath}`}>
                                <button className="rnd-btn" onClick={() => void deployCheckout()} disabled={!executionEnabled}>
                                    <IconGit size={13} />
                                    Checked-out branch
                                </button>
                            </Tooltip>
                        )}
                        <Tooltip label={lastGood ? `Prefill from #${lastGood.id}` : `No successful ${runWord.toLowerCase()} to repeat yet`}>
                            <span className="rnd-btn-wrap">
                                <button
                                    className="rnd-btn"
                                    onClick={() =>
                                        lastGood && openForm(lastGoodBranch ?? undefined, reusableOptions(lastGood.job?.options, secureNames))
                                    }
                                    disabled={!lastGood || !executionEnabled}>
                                    <IconFetch size={13} />
                                    {runsBranch ? `Redeploy${lastGoodBranch ? ` ${lastGoodBranch}` : ""}` : "Run again"}
                                </button>
                            </span>
                        </Tooltip>
                        <button className={`rnd-btn ${isProd ? "prod" : "primary"}`} onClick={() => openForm(undefined)} disabled={!executionEnabled}>
                            <IconRun size={11} />
                            {runWord}
                        </button>
                    </>
                }
                onRefresh={() => void execs.refresh()}
                refreshing={execs.status === "loading"}
            />
            <div className="rnd-service">
                {detail.data?.description && <p className="rnd-service-desc">{detail.data.description}</p>}
                {(!executionEnabled || detail.data?.node_filter) && (
                    <div className="rnd-insp-tags">
                        {!executionEnabled && <Tag tone="warn">executions disabled</Tag>}
                        {detail.data?.node_filter && <Tag>nodes: {detail.data.node_filter}</Tag>}
                    </div>
                )}
                {actionError && <div className="rnd-banner danger">{actionError}</div>}

                <dl className="rnd-facts">
                    {runsBranch && (
                        <div>
                            <dt>Live</dt>
                            <dd className="mono">{lastGoodBranch ?? "—"}</dd>
                        </div>
                    )}
                    <div>
                        <dt>{runsBranch ? "Deployed" : "Last success"}</dt>
                        <dd>
                            {lastGood && endedAt(lastGood) ? formatAgo(endedAt(lastGood)!, now) : "—"}
                            {lastGood?.user && <small> by {lastGood.user}</small>}
                        </dd>
                    </div>
                    <div>
                        <dt>Success</dt>
                        <dd>
                            {finished.length ? `${Math.round((succeeded / finished.length) * 100)}%` : "—"}
                            {finished.length > 0 && <small> of {finished.length}</small>}
                        </dd>
                    </div>
                    <div>
                        <dt>Usual time</dt>
                        <dd>{usual !== null ? formatDuration(usual * 1000) : "—"}</dd>
                    </div>
                    {range && (
                        <div>
                            <dt>Fastest · slowest</dt>
                            <dd>
                                {formatDuration(range[0] * 1000)} · {formatDuration(range[1] * 1000)}
                            </dd>
                        </div>
                    )}
                </dl>

                {running && (
                    <LiveRun
                        run={running}
                        stepNames={detail.data?.steps ?? []}
                        usual={usual}
                        now={now}
                        branch={branchFor(running)}
                        onOpen={() => openRun(running)}
                    />
                )}

                {execs.error && !execs.data && (
                    <EmptyState
                        tone="error"
                        icon={<IconWarning size={14} />}
                        title="Couldn't load runs"
                        message={execs.error}
                        action={{ label: "Retry", onClick: () => void execs.refresh() }}
                    />
                )}
                {!execs.data && !execs.error && <SkeletonRows rows={5} label="Loading runs" />}
                {execs.data && runs.length === 0 && <EmptyState message={`${job.name} hasn't run yet.`} />}

                {runs.length > 0 && (
                    <>
                        <section className="rnd-card rnd-service-card">
                            <div className="rnd-card-head">
                                <b>{runsBranch ? "Deploy times" : "Run times"}</b>
                                <span className="rnd-dim">
                                    last {Math.min(runs.length, HISTORY)} {runs.length === 1 ? "run" : "runs"}
                                </span>
                            </div>
                            <DeployTimes runs={runs} usual={usual} now={now} branchOf={branchFor} onOpen={openRun} />
                        </section>
                        <section className="rnd-card rnd-service-card">
                            <div className="rnd-card-head">
                                <b>History</b>
                                <span className="rnd-dim">{runs.length}</span>
                            </div>
                            <RunHistory runs={runs} branchOf={branchFor} usualOf={() => usual} now={now} showJob={false} onOpen={openRun} />
                        </section>
                    </>
                )}
            </div>
        </div>
    );
}

function LiveRun({
    run,
    stepNames,
    usual,
    now,
    branch,
    onOpen,
}: {
    run: RundeckExecution;
    stepNames: string[];
    usual: number | null;
    now: number;
    branch: string | null;
    onOpen: () => void;
}) {
    const steps = run.workflowState?.steps ?? [];
    const progress = executionProgress(run.workflowState);
    const current = progress ? Math.min(progress.total, progress.completed + 1) : null;
    const elapsed = runSeconds(run, now);
    return (
        <button className="rnd-card rnd-live-run" onClick={onOpen} title={`Open run #${run.id}`}>
            <span className="rnd-live-run-top">
                <span className="rnd-live-dot" />
                <b>
                    {branch ? "Deploying " : "Running"}
                    {branch && <span className="mono">{branch}</span>}
                </b>
                <span className="rnd-dim">
                    #{run.id} · {run.user ?? ""}
                </span>
                <span className="rnd-grow" />
                <span className="rnd-dim">
                    {current && progress ? `step ${current} of ${progress.total} · ` : ""}
                    {formatDuration(elapsed * 1000)}
                    {usual !== null ? ` of about ${formatDuration(usual * 1000)}` : ""}
                </span>
            </span>
            {steps.length > 0 && (
                <>
                    <span className="rnd-progress-track">
                        {steps.map((step, i) => (
                            <i key={step.stepctx ?? i} className={stepClass(step.executionState)} />
                        ))}
                    </span>
                    <span className="rnd-live-run-steps">
                        {steps.map((step, i) => (
                            <span key={step.stepctx ?? i} className={stepClass(step.executionState) === "running" ? "on" : undefined}>
                                {stepNames[i] || `step ${i + 1}`}
                            </span>
                        ))}
                    </span>
                </>
            )}
        </button>
    );
}

function stepClass(state: string | null | undefined): "pending" | "running" | "ok" | "fail" | "skip" {
    switch ((state ?? "").toUpperCase()) {
        case "RUNNING":
        case "RUNNING_HANDLER":
            return "running";
        case "SUCCEEDED":
            return "ok";
        case "FAILED":
        case "ABORTED":
            return "fail";
        case "NOT_ELIGIBLE":
        case "SKIPPED":
            return "skip";
        default:
            return "pending";
    }
}
