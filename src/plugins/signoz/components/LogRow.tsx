import type { LogLine } from "../api";

const clock = new Intl.DateTimeFormat(undefined, { hour: "2-digit", minute: "2-digit", second: "2-digit", hour12: false });

export function logTime(timestamp: string): string {
    const date = new Date(timestamp);
    if (Number.isNaN(date.getTime())) return timestamp;
    return `${clock.format(date)}.${String(date.getMilliseconds()).padStart(3, "0")}`;
}

export function severityTone(severity: string | null): "danger" | "warn" | "quiet" {
    const level = (severity ?? "").toUpperCase();
    if (level.startsWith("ERR") || level.startsWith("FATAL") || level.startsWith("CRIT")) return "danger";
    if (level.startsWith("WARN")) return "warn";
    return "quiet";
}

function valueText(value: unknown): string {
    return typeof value === "string" ? value : JSON.stringify(value);
}

interface Props {
    line: LogLine;
    expanded: boolean;
    onToggle: () => void;
    onOpenTrace?: (traceId: string) => void;
    /** Offered beside each attribute, so a filter is built from the value in front of you. */
    onFilter?: (key: string, value: string, keep: boolean) => void;
    showService?: boolean;
}

export function LogRow({ line, expanded, onToggle, onOpenTrace, onFilter, showService = true }: Props) {
    const attributes = Object.entries(line.attributes).filter(([, value]) => value !== "" && value !== null);
    return (
        <div className={`sgz-log${expanded ? " open" : ""}`}>
            <button type="button" className="sgz-log-head" onClick={onToggle} aria-expanded={expanded}>
                <span className="sgz-log-time">{logTime(line.timestamp)}</span>
                <span className={`sgz-sev ${severityTone(line.severity)}`}>{(line.severity ?? "").slice(0, 5) || "·"}</span>
                {showService && <span className="sgz-log-service">{line.service ?? "-"}</span>}
                <span className="sgz-log-body">{line.body}</span>
            </button>
            {expanded && (
                <div className="sgz-log-detail">
                    {line.traceId && onOpenTrace && (
                        <button type="button" className="sgz-link" onClick={() => onOpenTrace(line.traceId!)}>
                            trace {line.traceId}
                        </button>
                    )}
                    {line.body.includes("\n") && <pre className="sgz-log-full">{line.body}</pre>}
                    {attributes.length === 0 ? (
                        <div className="sgz-muted">no attributes</div>
                    ) : (
                        <dl className="sgz-attrs">
                            {attributes.map(([key, value]) => (
                                <div key={key} className="sgz-attr">
                                    <dt>{key}</dt>
                                    <dd>{valueText(value)}</dd>
                                    {onFilter && (
                                        <span className="sgz-attr-actions">
                                            <button
                                                type="button"
                                                title={`Only lines where ${key} is this`}
                                                onClick={() => onFilter(key, valueText(value), true)}>
                                                =
                                            </button>
                                            <button
                                                type="button"
                                                title={`Hide lines where ${key} is this`}
                                                onClick={() => onFilter(key, valueText(value), false)}>
                                                ≠
                                            </button>
                                        </span>
                                    )}
                                </div>
                            ))}
                        </dl>
                    )}
                </div>
            )}
        </div>
    );
}
