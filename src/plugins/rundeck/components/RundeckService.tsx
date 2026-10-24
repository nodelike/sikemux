import { useState } from "react";
import { currentBranch, git, openUrl, swallow } from "../../../plugin-api/host";
import { useNow } from "../../../plugin-api/hooks";
import { errorText, formatAgo, formatDuration } from "../../../plugin-api/format";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { EmptyState, IconExternal, IconFetch, IconGit, IconRun, IconWarning, SkeletonRows, Tooltip } from "../../../plugin-api/ui";
import { rndJobV } from "../resources";
import * as cmd from "../state";
import type { JobRef } from "../state";
import type { RundeckLiveRun, RundeckRun } from "../types.generated";
import { FolderChip, Header, Tag, levelCrumbs } from "./parts";
import { DeployTimes, RunHistory } from "./RunHistory";

interface Props {
    paneId: string;
    active: boolean;
    level: { kind: "service" } & JobRef;
}

const HISTORY = 30;

export function RundeckService({ paneId, active, level }: Props) {
    const job = cmd.jobRefOf(level);
    const page = useResourceEnabled(active, rndJobV, paneId, job.jobId, job.project);
    const data = page.data;
    const [actionError, setActionError] = useState<string | null>(null);

    const runs = data?.runs ?? [];
    const running = data?.live ?? null;
    const now = useNow(active && !!running);
    const runsBranch = data?.runsBranch ?? false;
    const runWord = data?.runWord ?? "Run";
    const isProd = data?.prod ?? false;
    const executionEnabled = data?.executionEnabled !== false;
    const lastGood = data?.lastGood ?? null;
    const lastGoodBranch = lastGood?.branch ?? null;
    const finished = data?.finished ?? 0;
    const usual = data?.usualSecs ?? null;
    const range = data?.fastestSecs != null && data.slowestSecs != null ? [data.fastestSecs, data.slowestSecs] : null;

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
            setActionError(errorText(e));
        }
    };
    const openRun = (executionId: number) => cmd.rundeckPush(paneId, { kind: "execution", ...job, executionId });

    return (
        <div className="rnd-main">
            <Header
                paneId={paneId}
                crumbs={levelCrumbs(paneId, level, job.project, job.group)}
                title={job.name}
                aside={<FolderChip project={job.project} group={job.group} prod={isProd} />}
                tools={
                    <>
                        {data?.permalink && (
                            <Tooltip label="Open in Rundeck">
                                <button
                                    className="rnd-btn rnd-icon-btn"
                                    onClick={() => void openUrl(data.permalink!).catch(swallow("open Rundeck URL"))}
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
                        <Tooltip label={lastGood ? `Prefill from #${lastGood.executionId}` : `No successful ${runWord.toLowerCase()} to repeat yet`}>
                            <span className="rnd-btn-wrap">
                                <button
                                    className="rnd-btn"
                                    onClick={() => lastGood && openForm(lastGoodBranch ?? undefined, data?.repeatOptions)}
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
                onRefresh={() => void page.refresh()}
                refreshing={page.status === "loading"}
            />
            <div className="rnd-service">
                {data?.description && <p className="rnd-service-desc">{data.description}</p>}
                {(!executionEnabled || data?.nodeFilter) && (
                    <div className="rnd-insp-tags">
                        {!executionEnabled && <Tag tone="warn">executions disabled</Tag>}
                        {data?.nodeFilter && <Tag>nodes: {data.nodeFilter}</Tag>}
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
                            {lastGood?.endedAt ? formatAgo(lastGood.endedAt, now) : "—"}
                            {lastGood?.user && <small> by {lastGood.user}</small>}
                        </dd>
                    </div>
                    <div>
                        <dt>Success</dt>
                        <dd>
                            {finished ? `${Math.round(((data?.succeeded ?? 0) / finished) * 100)}%` : "—"}
                            {finished > 0 && <small> of {finished}</small>}
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

                {running && <LiveRun run={running} usual={usual} now={now} onOpen={() => openRun(running.executionId)} />}

                {page.error && !data && (
                    <EmptyState
                        tone="error"
                        icon={<IconWarning size={14} />}
                        title="Couldn't load runs"
                        message={page.error}
                        action={{ label: "Retry", onClick: () => void page.refresh() }}
                    />
                )}
                {!data && !page.error && <SkeletonRows rows={5} label="Loading runs" />}
                {data && runs.length === 0 && <EmptyState message={`${job.name} hasn't run yet.`} />}

                {runs.length > 0 && (
                    <>
                        <section className="rnd-card rnd-service-card">
                            <div className="rnd-card-head">
                                <b>{runsBranch ? "Deploy times" : "Run times"}</b>
                                <span className="rnd-dim">
                                    last {Math.min(runs.length, HISTORY)} {runs.length === 1 ? "run" : "runs"}
                                </span>
                            </div>
                            <DeployTimes runs={runs} usual={usual} now={now} onOpen={(run: RundeckRun) => openRun(run.executionId)} />
                        </section>
                        <section className="rnd-card rnd-service-card">
                            <div className="rnd-card-head">
                                <b>History</b>
                                <span className="rnd-dim">{runs.length}</span>
                            </div>
                            <RunHistory runs={runs} now={now} showJob={false} onOpen={(run) => openRun(run.executionId)} />
                        </section>
                    </>
                )}
            </div>
        </div>
    );
}

function LiveRun({ run, usual, now, onOpen }: { run: RundeckLiveRun; usual: number | null; now: number; onOpen: () => void }) {
    const current = run.total ? Math.min(run.total, run.completed + 1) : null;
    const started = Date.parse(run.startedAt ?? "");
    const elapsed = Number.isNaN(started) ? 0 : Math.max(0, (now - started) / 1000);
    return (
        <button className="rnd-card rnd-live-run" onClick={onOpen} title={`Open run #${run.executionId}`}>
            <span className="rnd-live-run-top">
                <span className="rnd-live-dot" />
                <b>
                    {run.branch ? "Deploying " : "Running"}
                    {run.branch && <span className="mono">{run.branch}</span>}
                </b>
                <span className="rnd-dim">
                    #{run.executionId} · {run.user ?? ""}
                </span>
                <span className="rnd-grow" />
                <span className="rnd-dim">
                    {current ? `step ${current} of ${run.total} · ` : ""}
                    {formatDuration(elapsed * 1000)}
                    {usual !== null ? ` of about ${formatDuration(usual * 1000)}` : ""}
                </span>
            </span>
            {run.steps.length > 0 && (
                <>
                    <span className="rnd-progress-track">
                        {run.steps.map((step, i) => (
                            <i key={i} className={step.state} />
                        ))}
                    </span>
                    <span className="rnd-live-run-steps">
                        {run.steps.map((step, i) => (
                            <span key={i} className={step.state === "running" ? "on" : undefined}>
                                {step.name}
                            </span>
                        ))}
                    </span>
                </>
            )}
        </button>
    );
}
