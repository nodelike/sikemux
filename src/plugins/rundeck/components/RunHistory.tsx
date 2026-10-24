import { formatDuration } from "../../../plugin-api/format";
import { Tooltip } from "../../../plugin-api/ui";
import type { RundeckRun } from "../types.generated";
import { BranchChip, Status } from "./parts";
import { byDay, clockTime, runSecs } from "./runStats";

const initials = (user: string | null) =>
    (user ?? "?")
        .split(/[.\s_-]+/)
        .filter(Boolean)
        .slice(0, 2)
        .map((part) => part[0]!.toUpperCase())
        .join("");

/** Runs grouped by day. Each bar is the run's time, with a tick at that job's usual time. */
export function RunHistory({ runs, now, showJob, onOpen }: { runs: RundeckRun[]; now: number; showJob: boolean; onOpen: (run: RundeckRun) => void }) {
    return (
        <div className={`rnd-history${showJob ? " with-job" : ""}`} role="list">
            {byDay(runs).map(({ day, runs: dayRuns }) => (
                <div key={day} role="presentation">
                    <div className="rnd-history-day">
                        {day}
                        <span>{dayRuns.length}</span>
                    </div>
                    {dayRuns.map((ex) => {
                        const seconds = runSecs(ex, now);
                        const usual = ex.usualSecs;
                        const scale = Math.max((usual ?? seconds) * 1.7, seconds, 1);
                        const live = ex.live;
                        return (
                            <button
                                key={ex.executionId}
                                className="rnd-history-row"
                                role="listitem"
                                onClick={() => onOpen(ex)}
                                title={`Open run #${ex.executionId}`}>
                                <Status status={ex.status} label="" />
                                <span className="rnd-history-id">#{ex.executionId}</span>
                                {showJob && <span className="rnd-history-job">{ex.jobName ?? "—"}</span>}
                                <span className="rnd-history-branch">
                                    <BranchChip branch={ex.branch} />
                                </span>
                                <span className="rnd-history-who">
                                    <span className="rnd-avatar" aria-hidden="true">
                                        {initials(ex.user)}
                                    </span>
                                    <span>{ex.user ?? "—"}</span>
                                </span>
                                <span className="rnd-history-at">{clockTime(ex.startedAt)}</span>
                                <span className="rnd-dbar" aria-hidden="true">
                                    <i className={ex.statusKind} style={{ width: `${Math.min(100, (seconds / scale) * 100)}%` }} />
                                    {usual !== null && <b style={{ left: `${(usual / scale) * 100}%` }} />}
                                </span>
                                <span className={`rnd-history-took${live ? " live" : ""}`}>{live ? "running" : formatDuration(seconds * 1000)}</span>
                            </button>
                        );
                    })}
                </div>
            ))}
        </div>
    );
}

const SLOTS = 30;

/** One bar per run, oldest on the left, against a dashed line at the usual time. */
export function DeployTimes({
    runs,
    usual,
    now,
    onOpen,
}: {
    runs: RundeckRun[];
    usual: number | null;
    now: number;
    onOpen: (run: RundeckRun) => void;
}) {
    const shown = runs.slice(0, SLOTS).reverse();
    const longest = Math.max(60, (usual ?? 0) * 1.2, ...shown.map((ex) => runSecs(ex, now)));
    const step = longest > 1200 ? 600 : longest > 480 ? 300 : longest > 180 ? 120 : 60;
    const top = Math.ceil(longest / step) * step;
    const ticks = Array.from({ length: top / step + 1 }, (_, i) => i * step);
    return (
        <div className="rnd-chart">
            <div className="rnd-chart-axis" aria-hidden="true">
                {ticks.map((tick) => (
                    <span key={tick} style={{ bottom: `${(tick / top) * 100}%` }}>
                        {tick / 60}m
                    </span>
                ))}
            </div>
            <div className="rnd-chart-plot">
                {ticks.slice(1).map((tick) => (
                    <i key={tick} className="rnd-chart-grid" style={{ bottom: `${(tick / top) * 100}%` }} />
                ))}
                <div className="rnd-chart-cols">
                    {Array.from({ length: SLOTS - shown.length }, (_, i) => (
                        <span key={`empty-${i}`} className="rnd-chart-col empty" />
                    ))}
                    {shown.map((ex) => {
                        const seconds = runSecs(ex, now);
                        const branch = ex.branch;
                        return (
                            <Tooltip
                                key={ex.executionId}
                                side="top"
                                label={`#${ex.executionId}${branch ? ` · ${branch}` : ""} · ${formatDuration(seconds * 1000)} · ${ex.user ?? ""}`}>
                                <button className="rnd-chart-col" onClick={() => onOpen(ex)} aria-label={`Open run #${ex.executionId}`}>
                                    <i className={ex.statusKind} style={{ height: `${Math.max(3, (seconds / top) * 100)}%` }} />
                                </button>
                            </Tooltip>
                        );
                    })}
                </div>
                {usual !== null && (
                    <div className="rnd-chart-usual" style={{ bottom: `${(usual / top) * 100}%` }}>
                        <span>usual {formatDuration(usual * 1000)}</span>
                    </div>
                )}
            </div>
        </div>
    );
}
