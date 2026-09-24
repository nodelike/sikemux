import { useMemo } from "react";
import type { MatrixCell } from "../api";
import * as cmd from "../state";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { useActiveProjectCwd } from "../../../plugin-api/host";
import { rndMatrixR } from "../resources";
import { childSegment, inGroup, isLiveStatus } from "../shape";
import { EmptyState, IconRundeck, IconSearch, IconWarning } from "../../../plugin-api/ui";
import { useNow } from "./hooks";
import { RundeckMatrixRow } from "./RundeckMatrixRow";

interface Props {
    paneId: string;
    active: boolean;
}

interface RowGroup {
    segment: string | null;
    cells: MatrixCell[];
}

export function RundeckMatrix({ paneId, active }: Props) {
    const project = cmd.rundeckSettings.useSelect((s) => s.activeProject);
    const activeGroup = cmd.rundeckSettings.useSelect((s) => s.activeGroup);
    const branchOptions = cmd.rundeckSettings.useSelect((s) => s.branchOptions);
    const activeCwd = useActiveProjectCwd();

    const res = useResourceEnabled(active && !!project, rndMatrixR, project, branchOptions);
    const data = res.data;

    const cells = useMemo(() => {
        const list = (data?.cells ?? []).filter((c) => inGroup(c.group, activeGroup));
        return list.sort((a, b) => a.name.localeCompare(b.name));
    }, [data, activeGroup]);

    const groups = useMemo<RowGroup[]>(() => {
        const map = new Map<string | null, MatrixCell[]>();
        for (const cell of cells) {
            const segment = childSegment(cell.group, activeGroup);
            map.set(segment, [...(map.get(segment) ?? []), cell]);
        }
        return [...map.entries()]
            .sort(([a], [b]) => (a === null ? -1 : b === null ? 1 : a.localeCompare(b)))
            .map(([segment, list]) => ({ segment, cells: list }));
    }, [cells, activeGroup]);

    const anyLive = cells.some((c) => isLiveStatus(c.latest?.status));
    const now = useNow(active, anyLive ? 1_000 : 30_000);
    const loading = res.status === "loading";
    const timedOut = cells.filter((c) => c.error === "timed out").length;

    if (!project) {
        return <EmptyState icon={<IconRundeck size={14} />} message="Pick a Rundeck project from the tree." />;
    }

    const groupPath = (segment: string) => (activeGroup ? `${activeGroup}/${segment}` : segment);

    return (
        <div className="rnd-list">
            <div className="rnd-list-toolbar">
                <span className="rnd-list-meta">
                    <span className="rnd-list-meta-n">{cells.length}</span>
                    <span className="rnd-list-meta-l">jobs</span>
                    <span className="rnd-list-meta-sep">·</span>
                    <span className="rnd-list-meta-v">{activeGroup ? `${project} / ${activeGroup}` : project}</span>
                    {data && !loading && (
                        <>
                            <span className="rnd-list-meta-sep">·</span>
                            <span className="rnd-list-meta-l">{data.elapsed_ms}ms</span>
                        </>
                    )}
                    {loading && (
                        <>
                            <span className="rnd-list-meta-sep">·</span>
                            <span className="rnd-spinner inline" />
                            <span className="rnd-list-meta-l">refreshing</span>
                        </>
                    )}
                </span>
                <div className="rnd-list-tools">
                    <button className="rnd-btn-sm" onClick={cmd.openRundeckJobPalette} title="Search jobs in every project">
                        <IconSearch size={12} />
                        search
                    </button>
                    <button className="rnd-btn-sm" onClick={() => void res.refresh()} disabled={loading} title="Refresh">
                        refresh
                    </button>
                </div>
            </div>

            {data?.error && <div className="rnd-banner warn">{data.error}</div>}
            {data?.partial && (
                <div className="rnd-banner warn">
                    Rundeck was slow to answer{timedOut ? `: ${timedOut} job${timedOut === 1 ? "" : "s"} timed out` : ""}. Those rows show no recent
                    runs — refresh to try again.
                </div>
            )}
            {res.error && !data && (
                <EmptyState
                    tone="error"
                    icon={<IconWarning size={14} />}
                    title="Couldn't load jobs"
                    message={res.error}
                    action={{ label: "Retry", onClick: () => void res.refresh() }}
                />
            )}

            <div className="rnd-list-rows">
                {data && cells.length === 0 && <EmptyState message={activeGroup ? `No jobs in ${activeGroup}.` : `No jobs in ${project}.`} />}
                {groups.map((g) => (
                    <div className="rnd-group" key={g.segment ?? "\u0000"}>
                        {g.segment !== null && (
                            <div className="rnd-group-head">
                                <button
                                    type="button"
                                    className="rnd-group-link"
                                    onClick={() => cmd.selectRundeckGroup(paneId, project, groupPath(g.segment!))}>
                                    <span className="rnd-group-folder">{g.segment}/</span>
                                    <span className="rnd-group-count">{g.cells.length}</span>
                                </button>
                            </div>
                        )}
                        {g.cells.map((c) => (
                            <RundeckMatrixRow key={c.job_id} paneId={paneId} project={project} cell={c} activeCwd={activeCwd} now={now} />
                        ))}
                    </div>
                ))}
            </div>
        </div>
    );
}
