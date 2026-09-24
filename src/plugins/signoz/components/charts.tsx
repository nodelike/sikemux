import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { Series, VolumeBucket } from "../api";

/** Eight series at most get their own colour; the rest fold into "Other". */
export const SERIES_SLOTS = 8;
const OTHER = "Other";

export function useSize<T extends HTMLElement>(): [React.RefObject<T | null>, number, number] {
    const ref = useRef<T>(null);
    const [size, setSize] = useState({ width: 0, height: 0 });
    useEffect(() => {
        const element = ref.current;
        if (!element) return;
        const observer = new ResizeObserver(([entry]) =>
            setSize({ width: Math.floor(entry.contentRect.width), height: Math.floor(entry.contentRect.height) }),
        );
        observer.observe(element);
        return () => observer.disconnect();
    }, []);
    return [ref, size.width, size.height];
}

const clock = new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit", hour12: false });
const dayClock = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit", hour12: false });

export function timeLabel(ms: number, span: number): string {
    return span > 24 * 3_600_000 ? dayClock.format(ms) : clock.format(ms);
}

function compact(value: number): string {
    const magnitude = Math.abs(value);
    if (magnitude >= 1e9) return `${(value / 1e9).toFixed(1)}B`;
    if (magnitude >= 1e6) return `${(value / 1e6).toFixed(1)}M`;
    if (magnitude >= 1e4) return `${(value / 1e3).toFixed(1)}k`;
    if (magnitude >= 100 || Number.isInteger(value)) return String(Math.round(value));
    return String(Number(value.toPrecision(magnitude >= 1 ? 3 : 2)));
}

function duration(value: number, perSecond: number): string {
    const ms = value * perSecond;
    if (ms === 0) return "0";
    if (ms >= 60_000) return `${(ms / 60_000).toFixed(1)}m`;
    if (ms >= 1_000) return `${(ms / 1_000).toFixed(ms >= 10_000 ? 0 : 1)}s`;
    if (ms >= 1) return `${ms.toFixed(ms >= 100 ? 0 : 1)}ms`;
    return `${Math.round(ms * 1_000)}µs`;
}

function bytes(value: number): string {
    const units = ["B", "KB", "MB", "GB", "TB"];
    let scaled = value;
    let unit = 0;
    while (Math.abs(scaled) >= 1024 && unit < units.length - 1) {
        scaled /= 1024;
        unit += 1;
    }
    return `${scaled.toFixed(scaled >= 100 || unit === 0 ? 0 : 1)} ${units[unit]}`;
}

/** Values in the unit a SigNoz panel declares; anything unknown is a plain number. */
export function formatValue(value: number | null | undefined, unit = ""): string {
    if (value === null || value === undefined || !Number.isFinite(value)) return "–";
    switch (unit) {
        case "ns":
            return duration(value, 1e-6);
        case "µs":
        case "us":
            return duration(value, 1e-3);
        case "ms":
            return duration(value, 1);
        case "s":
            return duration(value, 1_000);
        case "bytes":
        case "decbytes":
            return bytes(value);
        case "percent":
            return `${compact(value)}%`;
        case "percentunit":
            return `${compact(value * 100)}%`;
        case "reqps":
        case "rps":
            return `${compact(value)}/s`;
        default:
            return compact(value);
    }
}

/** At most eight series keep their identity; the smallest of the rest are summed into "Other". */
export function foldSeries(series: readonly Series[]): Series[] {
    if (series.length <= SERIES_SLOTS) return [...series];
    const total = (serie: Series) => serie.points.reduce((sum, [, value]) => sum + Math.abs(value), 0);
    const ranked = [...series].sort((left, right) => total(right) - total(left));
    const kept = ranked.slice(0, SERIES_SLOTS - 1);
    const sums = new Map<number, number>();
    for (const serie of ranked.slice(SERIES_SLOTS - 1)) {
        for (const [at, value] of serie.points) sums.set(at, (sums.get(at) ?? 0) + value);
    }
    const other = { label: OTHER, points: [...sums.entries()].sort((left, right) => left[0] - right[0]) };
    return [...kept.sort((left, right) => series.indexOf(left) - series.indexOf(right)), other];
}

export function seriesColor(index: number, label: string): string {
    return label === OTHER ? "var(--gray-600)" : `var(--sgz-series-${(index % SERIES_SLOTS) + 1})`;
}

function niceTicks(max: number, count = 3): number[] {
    if (max <= 0) return [0];
    const rough = max / count;
    const power = 10 ** Math.floor(Math.log10(rough));
    const step = [1, 2, 2.5, 5, 10].map((factor) => factor * power).find((candidate) => candidate >= rough) ?? rough;
    const ticks = [];
    for (let tick = 0; tick <= max + step * 0.001; tick += step) ticks.push(tick);
    if (ticks[ticks.length - 1] < max) ticks.push(ticks[ticks.length - 1] + step);
    return ticks;
}

const PAD = { top: 8, right: 8, bottom: 20, left: 44 };

interface Tip {
    x: number;
    y: number;
    content: ReactNode;
}

function Tooltip({ tip, width }: { tip: Tip | null; width: number }) {
    if (!tip) return null;
    const flip = tip.x > width - 200;
    return (
        <div
            className="sgz-tip"
            style={{ left: flip ? undefined : tip.x + 12, right: flip ? width - tip.x + 12 : undefined, top: Math.max(0, tip.y - 8) }}>
            {tip.content}
        </div>
    );
}

export function Legend({ series, colors }: { series: readonly Series[]; colors?: readonly string[] }) {
    if (series.length < 2) return null;
    return (
        <ul className="sgz-legend">
            {series.map((serie, index) => (
                <li key={`${serie.label}:${index}`}>
                    <span className="sgz-swatch" style={{ background: colors?.[index] ?? seriesColor(index, serie.label) }} />
                    {serie.label || "series"}
                </li>
            ))}
        </ul>
    );
}

const LEGEND_ALLOWANCE = 22;

/** Lines, or stacked bars, over time on one axis, filling the space it is given. */
export function TimeChart({
    series,
    unit,
    bars = false,
    area = false,
    colors,
}: {
    series: readonly Series[];
    unit: string;
    bars?: boolean;
    /** Shades under a single line, for a chart that shows one quantity. */
    area?: boolean;
    /** Colours that carry meaning, like red for errors, in place of the series order. */
    colors?: readonly string[];
}) {
    const [ref, width, space] = useSize<HTMLDivElement>();
    const [tip, setTip] = useState<Tip | null>(null);
    const folded = useMemo(() => foldSeries(series), [series]);
    const colorOf = (position: number, label: string) => colors?.[position] ?? seriesColor(position, label);
    const height = Math.max(60, space - (folded.length > 1 ? LEGEND_ALLOWANCE : 0));
    const times = useMemo(
        () => [...new Set(folded.flatMap((serie) => serie.points.map(([at]) => at)))].sort((left, right) => left - right),
        [folded],
    );
    const byTime = useMemo(() => folded.map((serie) => new Map(serie.points)), [folded]);
    const stacks = useMemo(
        () => (bars ? times.map((at) => byTime.reduce((sum, points) => sum + Math.max(0, points.get(at) ?? 0), 0)) : []),
        [bars, byTime, times],
    );
    const max = bars ? Math.max(0, ...stacks) : Math.max(0, ...folded.flatMap((serie) => serie.points.map(([, value]) => value)));
    const ticks = niceTicks(max);
    const top = ticks[ticks.length - 1] || 1;
    const plotWidth = Math.max(0, width - PAD.left - PAD.right);
    const plotHeight = Math.max(0, height - PAD.top - PAD.bottom);
    const first = times[0] ?? 0;
    const last = times[times.length - 1] ?? first + 1;
    const span = Math.max(1, last - first);
    const x = (at: number) => PAD.left + ((at - first) / span) * plotWidth;
    const y = (value: number) => PAD.top + plotHeight - (value / top) * plotHeight;
    const slot = times.length > 1 ? plotWidth / times.length : plotWidth;

    const hover = (event: React.PointerEvent<SVGSVGElement>) => {
        if (times.length === 0) return;
        const box = event.currentTarget.getBoundingClientRect();
        const offset = event.clientX - box.left;
        const index = Math.min(times.length - 1, Math.max(0, Math.round(((offset - PAD.left) / Math.max(1, plotWidth)) * (times.length - 1))));
        const at = times[index];
        const rows = folded
            .map((serie, position) => ({ serie, position, value: byTime[position].get(at) }))
            .filter((row) => row.value !== undefined)
            .sort((left, right) => (right.value ?? 0) - (left.value ?? 0));
        setTip({
            x: x(at),
            y: event.clientY - box.top,
            content: (
                <>
                    <div className="sgz-tip-time">{timeLabel(at, span)}</div>
                    {rows.map((row) => (
                        <div key={row.position} className="sgz-tip-row">
                            <span className="sgz-swatch" style={{ background: colorOf(row.position, row.serie.label) }} />
                            <span className="sgz-tip-label">{row.serie.label || "value"}</span>
                            <span className="sgz-tip-value">{formatValue(row.value, unit)}</span>
                        </div>
                    ))}
                </>
            ),
        });
    };

    return (
        <div className="sgz-chart fill" ref={ref}>
            {width > 0 && times.length > 0 && (
                <svg width={width} height={height} onPointerMove={hover} onPointerLeave={() => setTip(null)} role="img" aria-label="Chart">
                    {ticks.map((tick) => (
                        <g key={tick}>
                            <line className="sgz-gridline" x1={PAD.left} x2={width - PAD.right} y1={y(tick)} y2={y(tick)} />
                            <text className="sgz-axis" x={PAD.left - 6} y={y(tick)} dy="0.32em" textAnchor="end">
                                {formatValue(tick, unit)}
                            </text>
                        </g>
                    ))}
                    {[first, first + span / 2, last].map((at, index) => (
                        <text
                            key={index}
                            className="sgz-axis"
                            x={x(at)}
                            y={height - 4}
                            textAnchor={index === 0 ? "start" : index === 2 ? "end" : "middle"}>
                            {timeLabel(at, span)}
                        </text>
                    ))}
                    {bars
                        ? times.map((at, index) => {
                              let base = 0;
                              const barWidth = Math.max(1, slot - 2);
                              return folded.map((serie, position) => {
                                  const value = Math.max(0, byTime[position].get(at) ?? 0);
                                  if (value === 0) return null;
                                  const top = y(base + value);
                                  const bottom = y(base);
                                  base += value;
                                  return (
                                      <rect
                                          key={`${index}:${position}`}
                                          x={PAD.left + index * slot + 1}
                                          y={top}
                                          width={barWidth}
                                          height={Math.max(0, bottom - top - (position > 0 ? 1 : 0))}
                                          rx={1}
                                          fill={colorOf(position, serie.label)}
                                      />
                                  );
                              });
                          })
                        : folded.map((serie, position) => {
                              const line = serie.points.map(([at, value]) => `${x(at)},${y(value)}`).join(" ");
                              const shade = area && folded.length === 1 && serie.points.length > 1;
                              const firstAt = serie.points[0]?.[0] ?? first;
                              const lastAt = serie.points[serie.points.length - 1]?.[0] ?? last;
                              return (
                                  <g key={position}>
                                      {shade && (
                                          <polygon
                                              className="sgz-area"
                                              points={`${x(firstAt)},${y(0)} ${line} ${x(lastAt)},${y(0)}`}
                                              fill={colorOf(position, serie.label)}
                                          />
                                      )}
                                      <polyline className="sgz-line" points={line} stroke={colorOf(position, serie.label)} />
                                  </g>
                              );
                          })}
                    {tip && <line className="sgz-crosshair" x1={tip.x} x2={tip.x} y1={PAD.top} y2={PAD.top + plotHeight} />}
                </svg>
            )}
            <Tooltip tip={tip} width={width} />
            <Legend series={folded} colors={colors} />
        </div>
    );
}

const LEVELS = [
    { key: "ERROR", match: ["ERROR", "FATAL", "CRITICAL", "EMERGENCY", "ALERT"], className: "danger", label: "errors" },
    { key: "WARN", match: ["WARN", "WARNING"], className: "warn", label: "warnings" },
    { key: "OTHER", match: [], className: "quiet", label: "the rest" },
] as const;

function levelOf(level: string) {
    return LEVELS.find((candidate) => (candidate.match as readonly string[]).includes(level)) ?? LEVELS[2];
}

/** How much logged, and how much of it failed, across the window. Dragging across it picks out a moment. */
export function VolumeChart({
    buckets,
    onZoom,
    height = 64,
}: {
    buckets: readonly VolumeBucket[];
    onZoom: (start: number, end: number) => void;
    height?: number;
}) {
    const [ref, width] = useSize<HTMLDivElement>();
    const [tip, setTip] = useState<Tip | null>(null);
    const [drag, setDrag] = useState<{ from: number; to: number } | null>(null);
    const grouped = useMemo(
        () =>
            buckets.map((bucket) => {
                const sums = { ERROR: 0, WARN: 0, OTHER: 0 };
                for (const [level, count] of Object.entries(bucket.counts)) sums[levelOf(level).key] += count;
                return { start: bucket.start, sums, total: sums.ERROR + sums.WARN + sums.OTHER };
            }),
        [buckets],
    );
    const max = Math.max(1, ...grouped.map((bucket) => bucket.total));
    const slot = grouped.length > 0 ? width / grouped.length : width;
    const step = grouped.length > 1 ? grouped[1].start - grouped[0].start : 60_000;
    const span = grouped.length > 0 ? grouped[grouped.length - 1].start + step - grouped[0].start : step;
    const indexAt = (event: React.PointerEvent) => {
        const box = (event.currentTarget as Element).getBoundingClientRect();
        return Math.min(grouped.length - 1, Math.max(0, Math.floor((event.clientX - box.left) / Math.max(1, slot))));
    };

    const onMove = (event: React.PointerEvent<SVGSVGElement>) => {
        if (grouped.length === 0) return;
        const index = indexAt(event);
        if (drag) setDrag({ ...drag, to: index });
        const bucket = grouped[index];
        setTip({
            x: index * slot + slot / 2,
            y: 0,
            content: (
                <>
                    <div className="sgz-tip-time">{timeLabel(bucket.start, span)}</div>
                    {LEVELS.map((level) => (
                        <div key={level.key} className="sgz-tip-row">
                            <span className={`sgz-swatch level-${level.className}`} />
                            <span className="sgz-tip-label">{level.label}</span>
                            <span className="sgz-tip-value">{bucket.sums[level.key]}</span>
                        </div>
                    ))}
                </>
            ),
        });
    };
    const onUp = () => {
        if (!drag) return;
        const from = Math.min(drag.from, drag.to);
        const to = Math.max(drag.from, drag.to);
        setDrag(null);
        if (to > from) onZoom(grouped[from].start, grouped[to].start + step);
    };

    return (
        <div className="sgz-volume" ref={ref}>
            {width > 0 && (
                <svg
                    width={width}
                    height={height}
                    onPointerDown={(event) => {
                        event.currentTarget.setPointerCapture(event.pointerId);
                        const index = indexAt(event);
                        setDrag({ from: index, to: index });
                    }}
                    onPointerMove={onMove}
                    onPointerUp={onUp}
                    onPointerLeave={() => setTip(null)}
                    role="img"
                    aria-label="Log lines over time, by level. Drag across it to look at one moment.">
                    {grouped.map((bucket, index) => {
                        let base = 0;
                        return LEVELS.map((level, position) => {
                            const value = bucket.sums[level.key];
                            if (value === 0) return null;
                            const barHeight = (value / max) * (height - 2);
                            const y = height - base - barHeight;
                            base += barHeight;
                            return (
                                <rect
                                    key={`${index}:${level.key}`}
                                    className={`level-${level.className}`}
                                    x={index * slot + 1}
                                    y={y}
                                    width={Math.max(1, slot - 2)}
                                    height={Math.max(1, barHeight - (position > 0 ? 1 : 0))}
                                    rx={1}
                                />
                            );
                        });
                    })}
                    {drag && drag.to !== drag.from && (
                        <rect
                            className="sgz-brush"
                            x={Math.min(drag.from, drag.to) * slot}
                            y={0}
                            width={(Math.abs(drag.to - drag.from) + 1) * slot}
                            height={height}
                        />
                    )}
                </svg>
            )}
            <Tooltip tip={tip} width={width} />
        </div>
    );
}

export function StatValue({ value, unit }: { value: number | null; unit: string }) {
    return <div className="sgz-stat">{formatValue(value, unit)}</div>;
}

/** Part of a whole, as ranked bars: easier to compare than slices of a pie. */
export function BarList({ rows, unit }: { rows: { label: string; value: number }[]; unit: string }) {
    const max = Math.max(1, ...rows.map((row) => row.value));
    return (
        <ol className="sgz-barlist">
            {rows.map((row, index) => (
                <li key={`${row.label}:${index}`}>
                    <span className="sgz-barlist-label">{row.label}</span>
                    <span className="sgz-barlist-track">
                        <span
                            className="sgz-barlist-bar"
                            style={{ width: `${(row.value / max) * 100}%`, background: seriesColor(index, row.label) }}
                        />
                    </span>
                    <span className="sgz-barlist-value">{formatValue(row.value, unit)}</span>
                </li>
            ))}
        </ol>
    );
}

export function DataTable({ columns, rows, unit }: { columns: { name: string; aggregation: boolean }[]; rows: unknown[][]; unit: string }) {
    return (
        <div className="sgz-table-wrap">
            <table className="sgz-table">
                <thead>
                    <tr>
                        {columns.map((column, index) => (
                            <th key={index} className={column.aggregation ? "num" : undefined}>
                                {column.name.startsWith("__result") ? "value" : column.name}
                            </th>
                        ))}
                    </tr>
                </thead>
                <tbody>
                    {rows.map((row, rowIndex) => (
                        <tr key={rowIndex}>
                            {row.map((cell, cellIndex) => (
                                <td key={cellIndex} className={columns[cellIndex]?.aggregation ? "num" : undefined}>
                                    {typeof cell === "number" ? formatValue(cell, columns[cellIndex]?.aggregation ? unit : "") : String(cell ?? "")}
                                </td>
                            ))}
                        </tr>
                    ))}
                </tbody>
            </table>
        </div>
    );
}
