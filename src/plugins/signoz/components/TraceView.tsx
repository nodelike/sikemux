import { useState } from "react";
import { useResource } from "../../../plugin-api/resources";
import { EmptyState, IconChevron, SkeletonRows } from "../../../plugin-api/ui";
import { failureMessage } from "../api";
import { signozTraceLogsR, signozTraceR } from "../resources";
import { LogRow } from "./LogRow";

const MAX_INDENT = 16;
const TRACE_LOG_LOOKBACK_MINUTES = 24 * 60;

export function formatMs(ms: number): string {
    if (ms >= 1_000) return `${(ms / 1_000).toFixed(ms >= 10_000 ? 0 : 1)}s`;
    if (ms >= 1) return `${ms.toFixed(ms >= 100 ? 0 : 1)}ms`;
    return `${Math.round(ms * 1_000)}µs`;
}

export function TraceView({ traceId, onBack }: { traceId: string; onBack: () => void }) {
    const trace = useResource(signozTraceR, traceId);
    const logs = useResource(signozTraceLogsR, { traceId, minutes: TRACE_LOG_LOOKBACK_MINUTES, limit: 500 });
    const [expanded, setExpanded] = useState<ReadonlySet<string>>(new Set());
    const toggle = (id: string) =>
        setExpanded((current) => {
            const next = new Set(current);
            if (next.has(id)) next.delete(id);
            else next.add(id);
            return next;
        });

    const data = trace.data;
    const total = Math.max(data?.durationMs ?? 0, 0.001);
    const traceLines = [...(logs.data?.lines ?? [])].reverse();

    return (
        <div className="sgz-trace">
            <div className="sgz-trace-head">
                <button type="button" className="sgz-back" onClick={onBack}>
                    <IconChevron size={11} className="sgz-back-chev" />
                    Back
                </button>
                <span className="sgz-trace-id">{traceId}</span>
                {data && (
                    <>
                        {data.errorCount > 0 && (
                            <span className="sgz-sev danger">
                                {data.errorCount} {data.errorCount === 1 ? "error" : "errors"}
                            </span>
                        )}
                        <span className="sgz-trace-meta">
                            {formatMs(data.durationMs)} · {data.spans.length} spans · {data.services.join(", ")}
                        </span>
                    </>
                )}
            </div>
            {data?.truncated && <div className="sgz-banner">This trace has more spans than Sikemux reads at once; the latest ones are missing.</div>}
            <div className="sgz-trace-body">
                <section className="sgz-waterfall" aria-label="Spans">
                    {trace.status === "loading" && !data && <SkeletonRows rows={8} label="Loading spans" />}
                    {trace.error && <EmptyState tone="error" message={failureMessage(trace.error)} />}
                    {data && data.spans.length === 0 && <EmptyState message="No spans for this trace. The service may send logs but not traces." />}
                    {data?.spans.map((span) => (
                        <div key={span.spanId} className={`sgz-span${span.error ? " error" : ""}`} title={span.status ?? undefined}>
                            <span
                                className="sgz-span-label"
                                style={{ paddingLeft: `calc(var(--space-2) + ${Math.min(span.depth, MAX_INDENT)} * var(--space-2))` }}>
                                <span className="sgz-span-name">{span.name}</span>
                                <span className="sgz-span-service">{span.service}</span>
                            </span>
                            <span className="sgz-span-track">
                                <span
                                    className="sgz-span-bar"
                                    style={{ left: `${(span.offsetMs / total) * 100}%`, width: `max(1px, ${(span.durationMs / total) * 100}%)` }}
                                />
                            </span>
                            <span className="sgz-span-time">{formatMs(span.durationMs)}</span>
                        </div>
                    ))}
                </section>
                <section className="sgz-trace-logs" aria-label="Log lines in this trace">
                    <h3 className="sgz-section-label">Log lines in this trace</h3>
                    {logs.status === "loading" && !logs.data && <SkeletonRows rows={4} label="Loading log lines" />}
                    {logs.error && <EmptyState tone="error" message={failureMessage(logs.error)} />}
                    {logs.data && traceLines.length === 0 && <EmptyState message="No log lines carry this trace id." />}
                    {traceLines.map((line) => (
                        <LogRow key={line.id} line={line} expanded={expanded.has(line.id)} onToggle={() => toggle(line.id)} />
                    ))}
                </section>
            </div>
        </div>
    );
}
