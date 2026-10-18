import { formatDuration } from "../../../plugin-api/format";
import { Tooltip } from "../../../plugin-api/ui";
import type { RundeckExecution } from "../api";
import { isLiveStatus } from "../shape";
import { statusKind } from "./branchStyle";
import { BranchChip, Status } from "./parts";
import { byDay, clockTime, runSeconds, startedAt } from "./runStats";

const initials = (user: string | null) =>
    (user ?? "?")
        .split(/[.\s_-]+/)
        .filter(Boolean)
        .slice(0, 2)
        .map((part) => part[0]!.toUpperCase())
        .join("");

/** Runs grouped by day. Each bar is the run's time, with a tick at that job's usual time. */
export function RunHistory({
    runs,
    branchOf,
    usualOf,
    now,
    showJob,
    onOpen,
}: {
    runs: RundeckExecution[];
    branchOf: (ex: RundeckExecution) => string | null;
    usualOf: (ex: RundeckExecution) => number | null;
    now: number;
    showJob: boolean;
    onOpen: (ex: RundeckExecution) => void;
}) {
    return (
        <div className={`rnd-history${showJob ? " with-job" : ""}`} role="list">
            {byDay(runs).map(({ day, runs: dayRuns }) => (
                <div key={day} role="presentation">
                    <div className="rnd-history-day">
                        {day}
                        <span>{dayRuns.length}</span>
                    </div>
                    {dayRuns.map((ex) => {
                        const seconds = runSeconds(ex, now);
                        const usual = usualOf(ex);
                        const scale = Math.max((usual ?? seconds) * 1.7, seconds, 1);
                        const live = isLiveStatus(ex.status);
                        return (
                            <button key={ex.id} className="rnd-history-row" role="listitem" onClick={() => onOpen(ex)} title={`Open run #${ex.id}`}>
                                <Status status={ex.status} label="" />
                                <span className="rnd-history-id">#{ex.id}</span>
                                {showJob && <span className="rnd-history-job">{ex.job?.name ?? "—"}</span>}
                                <span className="rnd-history-branch">
                                    <BranchChip branch={branchOf(ex)} />
                                </span>
                                <span className="rnd-history-who">
                                    <span className="rnd-avatar" aria-hidden="true">
                                        {initials(ex.user)}
                                    </span>
                                    <span>{ex.user ?? "—"}</span>
                                </span>
                                <span className="rnd-history-at">{clockTime(startedAt(ex))}</span>
                                <span className="rnd-dbar" aria-hidden="true">
                                    <i className={statusKind(ex.status)} style={{ width: `${Math.min(100, (seconds / scale) * 100)}%` }} />
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
    branchOf,
    onOpen,
}: {
    runs: RundeckExecution[];
    usual: number | null;
    now: number;
    branchOf: (ex: RundeckExecution) => string | null;
    onOpen: (ex: RundeckExecution) => void;
}) {
    const shown = runs.slice(0, SLOTS).reverse();
    const longest = Math.max(60, (usual ?? 0) * 1.2, ...shown.map((ex) => runSeconds(ex, now)));
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
                        const seconds = runSeconds(ex, now);
                        const branch = branchOf(ex);
                        return (
                            <Tooltip
                                key={ex.id}
                                side="top"
                                label={`#${ex.id}${branch ? ` · ${branch}` : ""} · ${formatDuration(seconds * 1000)} · ${ex.user ?? ""}`}>
                                <button className="rnd-chart-col" onClick={() => onOpen(ex)} aria-label={`Open run #${ex.id}`}>
                                    <i className={statusKind(ex.status)} style={{ height: `${Math.max(3, (seconds / top) * 100)}%` }} />
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
