import { useState } from "react";
import { useActiveProjectCwd } from "../../../plugin-api/host";
import { useNow } from "../../../plugin-api/hooks";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { EmptyState, IconWarning, Seg, SkeletonRows } from "../../../plugin-api/ui";
import { rndActivityV } from "../resources";
import * as cmd from "../state";
import type { RundeckRun } from "../types.generated";
import { Header, levelCrumbs } from "./parts";
import { RunHistory } from "./RunHistory";

type ActivityFilter = "all" | "running" | "failed" | "mine";

const PAGE = 50;
const MOST = 200;

export function RundeckActivity({ paneId, project, active }: { paneId: string; project: string; active: boolean }) {
    const activeCwd = useActiveProjectCwd();
    const [max, setMax] = useState(PAGE);
    const [filter, setFilter] = useState<ActivityFilter>("all");
    const res = useResourceEnabled(active, rndActivityV, paneId, project, max);

    const runs = res.data?.runs ?? [];
    const now = useNow(active && (res.data?.running ?? 0) > 0, 5_000);
    const me = res.data?.me ?? null;
    const counts = { running: res.data?.running ?? 0, failed: res.data?.failed ?? 0, mine: res.data?.mine ?? 0 };
    const shown = runs.filter((ex) => {
        if (filter === "running") return ex.live;
        if (filter === "failed") return ex.statusKind === "failed";
        if (filter === "mine") return !!me && ex.user === me;
        return true;
    });

    const openRun = (ex: RundeckRun) => {
        if (!ex.jobId) return;
        const job = {
            project,
            jobId: ex.jobId,
            name: ex.jobName ?? ex.jobId,
            group: ex.jobGroup,
            repoPath: cmd.linkedRepoPath({ jobId: ex.jobId, name: ex.jobName ?? "" }, activeCwd),
        };
        cmd.rundeckPush(paneId, { kind: "execution", ...job, executionId: ex.executionId });
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
                        <RunHistory runs={shown} now={now} showJob onOpen={openRun} />
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
