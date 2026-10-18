import { useMemo, useState } from "react";
import { useActiveProjectCwd } from "../../../plugin-api/host";
import { useEvery, useNow } from "../../../plugin-api/hooks";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { EmptyState, IconWarning, Seg, SkeletonRows } from "../../../plugin-api/ui";
import type { RundeckExecution } from "../api";
import { rndProjectExecutionsR, rndStatusR, useBranchOptions } from "../resources";
import * as cmd from "../state";
import { branchOf, isLiveStatus } from "../shape";
import { statusKind } from "./branchStyle";
import { newestExecutions } from "./executionProgress";
import { Header, levelCrumbs } from "./parts";
import { RunHistory } from "./RunHistory";
import { usualSeconds } from "./runStats";

type ActivityFilter = "all" | "running" | "failed" | "mine";

const PAGE = 50;
const MOST = 200;

export function RundeckActivity({ paneId, project, active }: { paneId: string; project: string; active: boolean }) {
    const branchOptions = useBranchOptions();
    const activeCwd = useActiveProjectCwd();
    const [max, setMax] = useState(PAGE);
    const [filter, setFilter] = useState<ActivityFilter>("all");
    const status = useResourceEnabled(active, rndStatusR);
    const res = useResourceEnabled(active, rndProjectExecutionsR, project, max);

    const runs = useMemo(() => newestExecutions(res.data?.executions ?? []), [res.data]);
    const anyLive = runs.some((ex) => isLiveStatus(ex.status));
    const now = useNow(active && anyLive, 5_000);
    useEvery(active, anyLive ? 5_000 : 30_000, () => void res.refresh().catch(() => {}));

    const usualByJob = useMemo(() => {
        const byJob = new Map<string, RundeckExecution[]>();
        for (const ex of runs) {
            const id = ex.job?.id;
            if (id) byJob.set(id, [...(byJob.get(id) ?? []), ex]);
        }
        return new Map([...byJob].map(([id, list]) => [id, usualSeconds(list, Date.now())]));
    }, [runs]);

    const me = status.data?.user ?? null;
    const counts = {
        running: runs.filter((ex) => isLiveStatus(ex.status)).length,
        failed: runs.filter((ex) => statusKind(ex.status) === "failed").length,
        mine: me ? runs.filter((ex) => ex.user === me).length : 0,
    };
    const shown = runs.filter((ex) => {
        if (filter === "running") return isLiveStatus(ex.status);
        if (filter === "failed") return statusKind(ex.status) === "failed";
        if (filter === "mine") return !!me && ex.user === me;
        return true;
    });

    const openRun = (ex: RundeckExecution) => {
        if (!ex.job?.id) return;
        const job = {
            project,
            jobId: ex.job.id,
            name: ex.job.name ?? ex.job.id,
            group: ex.job.group || null,
            repoPath: cmd.linkedRepoPath({ jobId: ex.job.id, name: ex.job.name ?? "" }, activeCwd),
        };
        cmd.rundeckPush(paneId, { kind: "execution", ...job, executionId: ex.id });
    };
    const total = res.data?.total ?? null;
    const more = total !== null && runs.length < total && max < MOST;

    return (
        <div className="rnd-main">
            <Header
                paneId={paneId}
                crumbs={levelCrumbs(paneId, { kind: "activity", project }, project, null)}
                title="Activity"
                count={total ?? undefined}
                aside={
                    <Seg
                        label="Filter runs"
                        value={filter}
                        onChange={setFilter}
                        options={[
                            { value: "all", label: "All", count: runs.length },
                            { value: "running", label: "Running", count: counts.running },
                            { value: "failed", label: "Failed", count: counts.failed },
                            ...(me ? [{ value: "mine" as const, label: "Mine", count: counts.mine }] : []),
                        ]}
                    />
                }
                onRefresh={() => void res.refresh()}
                refreshing={res.status === "loading"}
            />
            <div className="rnd-service">
                {res.error && !res.data && (
                    <EmptyState
                        tone="error"
                        icon={<IconWarning size={14} />}
                        title="Couldn't load runs"
                        message={res.error}
                        action={{ label: "Retry", onClick: () => void res.refresh() }}
                    />
                )}
                {!res.data && !res.error && <SkeletonRows rows={8} label="Loading runs" />}
                {res.data && shown.length === 0 && (
                    <EmptyState message={runs.length ? "No runs match this filter." : `Nothing has run in ${project} yet.`} />
                )}
                {shown.length > 0 && (
                    <section className="rnd-card rnd-service-card">
                        <RunHistory
                            runs={shown}
                            branchOf={(ex) => branchOf(ex.job?.options, branchOptions)}
                            usualOf={(ex) => (ex.job?.id ? (usualByJob.get(ex.job.id) ?? null) : null)}
                            now={now}
                            showJob
                            onOpen={openRun}
                        />
                        {more && (
                            <div className="rnd-history-more">
                                <button
                                    className="rnd-btn"
                                    onClick={() => setMax((m) => Math.min(MOST, m + PAGE))}
                                    disabled={res.status === "loading"}>
                                    Show older runs
                                </button>
                            </div>
                        )}
                    </section>
                )}
            </div>
        </div>
    );
}
