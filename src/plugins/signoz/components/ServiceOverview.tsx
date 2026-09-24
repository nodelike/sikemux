import { useMemo } from "react";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { EmptyState, SkeletonRows } from "../../../plugin-api/ui";
import { failureMessage, type ErrorGroup, type Operation, type Scope, type Series, type ServiceOverview as Overview } from "../api";
import { perMinute, percent } from "../health";
import { signozErrorGroupsR, signozOperationsR, signozOverviewR } from "../resources";
import { addFilter, showService, updateView } from "../state";
import { TimeChart, formatValue, timeLabel } from "./charts";

const OPERATIONS_SHOWN = 12;

function windowMinutes(scope: Scope): number {
    if (scope.start !== undefined && scope.end !== undefined) return Math.max(1, (scope.end - scope.start) / 60_000);
    return scope.minutes ?? 15;
}

/** The fixed words of a pattern, which a text search can find again. */
export function searchableText(pattern: string): string {
    return pattern
        .split(/<n>|<id>/)
        .map((part) => part.trim())
        .sort((left, right) => right.length - left.length)[0];
}

function Stat({ label, value, tone }: { label: string; value: string; tone?: "bad" }) {
    return (
        <div className="sgz-tile">
            <span className="sgz-label">{label}</span>
            <span className={`sgz-tile-value${tone ? ` ${tone}` : ""}`}>{value}</span>
        </div>
    );
}

function ChartCard({ title, children }: { title: string; children: React.ReactNode }) {
    return (
        <section className="sgz-box sgz-chart-card">
            <h3 className="sgz-label">{title}</h3>
            <div className="sgz-box-chart">{children}</div>
        </section>
    );
}

function Charts({ overview }: { overview: Overview }) {
    const requests = useMemo<Series[]>(() => [{ label: "requests / min", points: overview.requests }], [overview.requests]);
    const failures = useMemo<Series[]>(() => [{ label: "errors / min", points: overview.failures }], [overview.failures]);
    const latency = useMemo<Series[]>(() => [{ label: "p99", points: overview.p99 }], [overview.p99]);
    return (
        <div className="sgz-chart-row">
            <ChartCard title="Requests / min">
                <TimeChart series={requests} unit="" area />
            </ChartCard>
            <ChartCard title="Errors / min">
                {overview.errors === 0 ? (
                    <div className="sgz-chart-empty">No errors in this window.</div>
                ) : (
                    <TimeChart series={failures} unit="" area colors={["var(--danger)"]} />
                )}
            </ChartCard>
            <ChartCard title="p99 latency">
                <TimeChart series={latency} unit="ms" area colors={["var(--sgz-series-4)"]} />
            </ChartCard>
        </div>
    );
}

function Operations({ paneId, operations, minutes }: { paneId: string; operations: Operation[]; minutes: number }) {
    if (operations.length === 0) return <div className="sgz-box-empty">No entry spans in this window.</div>;
    return (
        <table className="sgz-list-table">
            <thead>
                <tr>
                    <th>Endpoint</th>
                    <th className="num">Requests</th>
                    <th className="num">Errors</th>
                    <th className="num">p99</th>
                </tr>
            </thead>
            <tbody>
                {operations.slice(0, OPERATIONS_SHOWN).map((operation) => (
                    <tr
                        key={operation.name}
                        className="sgz-row-link"
                        title="Show its traces"
                        onClick={() => {
                            addFilter(paneId, { key: "name", op: "equals", value: operation.name });
                            updateView(paneId, { serviceTab: "traces" });
                        }}>
                        <td>
                            <button type="button" className="sgz-cell-link">
                                {operation.name}
                            </button>
                        </td>
                        <td className="num">{perMinute(operation.calls, minutes)}</td>
                        <td className={`num${operation.errors > 0 ? " bad" : " quiet"}`}>{percent(operation.errorRate)}</td>
                        <td className="num">{formatValue(operation.p99Ms, "ms")}</td>
                    </tr>
                ))}
            </tbody>
        </table>
    );
}

function Errors({ paneId, service, groups }: { paneId: string; service: string; groups: ErrorGroup[] }) {
    if (groups.length === 0) return <div className="sgz-box-empty">No error logs in this window.</div>;
    const span = Math.max(...groups.map((group) => group.lastSeen)) - Math.min(...groups.map((group) => group.firstSeen));
    return (
        <ul className="sgz-errors">
            {groups.map((group) => (
                <li key={group.pattern}>
                    <button
                        type="button"
                        className="sgz-error-row"
                        title={group.sample}
                        onClick={() => {
                            showService(paneId, service, "logs");
                            updateView(paneId, { severities: ["FATAL", "ERROR"], text: searchableText(group.pattern) });
                        }}>
                        <span className="sgz-error-text">{group.pattern}</span>
                        <span className="sgz-error-seen">last {timeLabel(group.lastSeen, span)}</span>
                        <span className="sgz-error-count">{group.count.toLocaleString()}</span>
                    </button>
                </li>
            ))}
        </ul>
    );
}

export function ServiceOverview({ paneId, service, scope, active }: { paneId: string; service: string; scope: Scope; active: boolean }) {
    const overview = useResourceEnabled(active, signozOverviewR, scope);
    const operations = useResourceEnabled(active, signozOperationsR, scope);
    const errors = useResourceEnabled(active, signozErrorGroupsR, scope);
    const minutes = windowMinutes(scope);

    if (overview.error) return <EmptyState tone="error" message={failureMessage(overview.error)} />;
    if (!overview.data) return <SkeletonRows rows={8} label={`Reading ${service}`} />;
    const data = overview.data;
    if (data.calls === 0) {
        return <EmptyState message={`${service} sent no traces in this window. Its logs may still be there.`} />;
    }

    return (
        <div className="sgz-overview">
            <div className="sgz-tiles">
                <Stat label="Requests" value={perMinute(data.calls, minutes)} />
                <Stat label="Error rate" value={percent(data.errorRate)} tone={data.errors > 0 ? "bad" : undefined} />
                <Stat label="p99 latency" value={formatValue(data.p99Ms, "ms")} />
                <Stat label="p50 latency" value={formatValue(data.p50Ms, "ms")} />
            </div>
            <Charts overview={data} />
            <div className="sgz-overview-lists">
                <section className="sgz-box sgz-list-card">
                    <h3 className="sgz-label">Endpoints</h3>
                    {operations.error ? (
                        <div className="sgz-box-empty sgz-error">{failureMessage(operations.error)}</div>
                    ) : operations.data ? (
                        <Operations paneId={paneId} operations={operations.data} minutes={minutes} />
                    ) : (
                        <SkeletonRows rows={4} label="Reading endpoints" />
                    )}
                </section>
                <section className="sgz-box sgz-list-card">
                    <h3 className="sgz-label">Errors</h3>
                    {errors.error ? (
                        <div className="sgz-box-empty sgz-error">{failureMessage(errors.error)}</div>
                    ) : errors.data ? (
                        <Errors paneId={paneId} service={service} groups={errors.data} />
                    ) : (
                        <SkeletonRows rows={4} label="Reading errors" />
                    )}
                </section>
            </div>
        </div>
    );
}
