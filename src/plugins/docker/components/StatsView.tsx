import { useEffect, useRef, useState } from "react";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { SkeletonRows } from "../../../plugin-api/ui";
import { formatBytes } from "../../../plugin-api/format";
import type { ContainerDetail, ContainerStats } from "../api";
import { percent } from "../format";
import { usePoll } from "../hooks";
import { dockerStatsR } from "../resources";

export const STATS_POLL_MS = 3_000;
/** About three minutes of samples at one every three seconds. */
const HISTORY = 60;

export function StatsView({ active, container }: { active: boolean; container: ContainerDetail }) {
    const stats = useResourceEnabled(active && container.state.running, dockerStatsR);
    usePoll(active && container.state.running, STATS_POLL_MS, stats.refresh);
    const now = stats.data?.find((each) => each.id === container.id);
    const history = useHistory(now, stats.loadedAt);

    if (!container.state.running) return <p className="docker-note">It isn't running, so it uses nothing. Start it to see its load.</p>;
    if (!now) return stats.status === "error" ? <p className="docker-note">{stats.error}</p> : <SkeletonRows rows={3} label="Measuring" />;
    const memoryShare = percent(now.memory, now.memoryLimit);
    return (
        <div className="docker-stats">
            <Meter label="CPU" value={`${now.cpuPercent.toFixed(1)}%`} detail="of one CPU" points={history.map((each) => each.cpuPercent)} />
            <Meter
                label="Memory"
                value={formatBytes(now.memory)}
                detail={`${memoryShare}% of ${formatBytes(now.memoryLimit)}`}
                points={history.map((each) => each.memory)}
                share={memoryShare}
            />
            <div className="docker-facts docker-stat-pairs" aria-label="Traffic">
                <h3>Since it started</h3>
                <Pair label="Network in" value={formatBytes(now.netRx)} />
                <Pair label="Network out" value={formatBytes(now.netTx)} />
                <Pair label="Disk read" value={formatBytes(now.diskRead)} />
                <Pair label="Disk written" value={formatBytes(now.diskWrite)} />
                <Pair label="Processes" value={String(now.pids)} />
            </div>
        </div>
    );
}

/** The samples seen while the view is open, newest last: one per reading, even when nothing changed. */
function useHistory(now: ContainerStats | undefined, readAt: number | undefined): ContainerStats[] {
    const [history, setHistory] = useState<ContainerStats[]>([]);
    const last = useRef<number | undefined>(undefined);
    useEffect(() => {
        if (!now || readAt === undefined || readAt === last.current) return;
        last.current = readAt;
        setHistory((before) => [...before, now].slice(-HISTORY));
    }, [now, readAt]);
    return history;
}

function Meter({ label, value, detail, points, share }: { label: string; value: string; detail: string; points: number[]; share?: number }) {
    return (
        <section className="docker-facts docker-meter" aria-label={label}>
            <h3>{label}</h3>
            <div className="docker-meter-value">
                <strong>{value}</strong>
                <span className="docker-muted">{detail}</span>
            </div>
            {share !== undefined ? (
                <div className="docker-bar" role="meter" aria-label={`${label} used`} aria-valuemin={0} aria-valuemax={100} aria-valuenow={share}>
                    <span style={{ width: `${share}%` }} />
                </div>
            ) : null}
            <Sparkline points={points} />
        </section>
    );
}

export function Sparkline({ points, width = 240, height = 36 }: { points: number[]; width?: number; height?: number }) {
    if (points.length < 2) return <div className="docker-sparkline-empty" style={{ height }} />;
    const top = Math.max(...points, 1e-9);
    const step = width / (points.length - 1);
    const path = points.map((value, index) => `${(index * step).toFixed(1)},${(height - (value / top) * (height - 2) - 1).toFixed(1)}`).join(" ");
    return (
        <svg className="docker-sparkline" viewBox={`0 0 ${width} ${height}`} preserveAspectRatio="none" aria-hidden="true">
            <polyline points={path} fill="none" stroke="currentColor" strokeWidth="1.5" vectorEffect="non-scaling-stroke" />
        </svg>
    );
}

function Pair({ label, value }: { label: string; value: string }) {
    return (
        <div className="docker-fact">
            <span className="docker-fact-label">{label}</span>
            <span className="docker-fact-value">{value}</span>
        </div>
    );
}
