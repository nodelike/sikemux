import { errorText } from "../../../plugin-api/format";
import { useEffect, useMemo, useRef, useState } from "react";
import { useDebounced } from "../../../plugin-api/hooks";
import { Dropdown, EmptyState, SkeletonRows } from "../../../plugin-api/ui";
import { gcpApi, type LogEntry, type LogQuery, type Target } from "../api";
import { Problem } from "./Problem";

const SEVERITIES = [
    { value: "", label: "All severities" },
    { value: "DEBUG", label: "Debug and above" },
    { value: "INFO", label: "Info and above" },
    { value: "WARNING", label: "Warning and above" },
    { value: "ERROR", label: "Error and above" },
    { value: "CRITICAL", label: "Critical and above" },
];

const SPANS = [
    { value: "15m", label: "Last 15 minutes" },
    { value: "1h", label: "Last hour" },
    { value: "6h", label: "Last 6 hours" },
    { value: "1d", label: "Last day" },
    { value: "7d", label: "Last 7 days" },
];

const MAX_ENTRIES = 2000;
const BUCKETS = 48;

const LOUD = new Set(["ERROR", "CRITICAL", "ALERT", "EMERGENCY"]);

/** Entries counted into even time slices, each split into errors, warnings and the rest. */
export function histogram(entries: { at: string; severity: string }[], buckets = BUCKETS): { errors: number; warnings: number; other: number }[] {
    const times = entries.map((entry) => Date.parse(entry.at)).filter((time) => Number.isFinite(time));
    const slices = Array.from({ length: buckets }, () => ({ errors: 0, warnings: 0, other: 0 }));
    if (times.length === 0) return slices;
    const start = Math.min(...times);
    const span = Math.max(1, Math.max(...times) - start);
    entries.forEach((entry) => {
        const time = Date.parse(entry.at);
        if (!Number.isFinite(time)) return;
        const slice = slices[Math.min(buckets - 1, Math.floor(((time - start) / span) * buckets))];
        if (LOUD.has(entry.severity)) slice.errors += 1;
        else if (entry.severity === "WARNING") slice.warnings += 1;
        else slice.other += 1;
    });
    return slices;
}

function Histogram({ entries }: { entries: LogEntry[] }) {
    const slices = histogram(entries);
    const most = Math.max(1, ...slices.map((slice) => slice.errors + slice.warnings + slice.other));
    return (
        <div className="gcp-histogram" role="img" aria-label={`${entries.length} entries over time`}>
            {slices.map((slice, index) => (
                <span key={index} className="gcp-histogram-bar">
                    <i className="other" style={{ height: `${(slice.other / most) * 100}%` }} />
                    <i className="warn" style={{ height: `${(slice.warnings / most) * 100}%` }} />
                    <i className="error" style={{ height: `${(slice.errors / most) * 100}%` }} />
                </span>
            ))}
        </div>
    );
}

function time(at: string): string {
    const when = new Date(at);
    return Number.isNaN(when.getTime())
        ? at
        : when.toLocaleTimeString(undefined, { hour12: false, hour: "2-digit", minute: "2-digit", second: "2-digit" });
}

function day(at: string): string {
    const when = new Date(at);
    return Number.isNaN(when.getTime()) ? "" : when.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

/** One entry's row, opening to its JSON payload, request and labels. */
function Entry({ entry }: { entry: LogEntry }) {
    const [open, setOpen] = useState(false);
    const extra = { http: entry.http, json: entry.json, labels: entry.labels, log: entry.log, trace: entry.trace };
    return (
        <div className={`gcp-log-entry${open ? " open" : ""}`}>
            <button type="button" className="gcp-log-row" onClick={() => setOpen(!open)} aria-expanded={open}>
                <span className={`gcp-severity ${entry.severity}`} title={entry.severity}>
                    {entry.severity.charAt(0)}
                </span>
                <span className="gcp-log-time" title={entry.at}>
                    <span className="gcp-muted">{day(entry.at)}</span> {time(entry.at)}
                </span>
                {entry.http && <span className={`gcp-http s${String(entry.http.status ?? "").charAt(0)}`}>{entry.http.status}</span>}
                <span className="gcp-log-text">{entry.text || <span className="gcp-muted">(no message)</span>}</span>
            </button>
            {open && <pre className="gcp-log-detail">{JSON.stringify(extra, (_key, value: unknown) => (value === null ? undefined : value), 2)}</pre>}
        </div>
    );
}

/**
 * Log entries for a query, as Logs Explorer lists them: a severity, a time range and words to find, newest last,
 * with a live mode that follows new entries as they arrive.
 */
export function LogStream({ active, target, query, extraFilter }: { active: boolean; target: Target; query: LogQuery; extraFilter?: string }) {
    const [severity, setSeverity] = useState("");
    const [since, setSince] = useState("1h");
    const [words, setWords] = useState("");
    const text = useDebounced(words.trim(), 400);
    const [live, setLive] = useState(false);
    const [entries, setEntries] = useState<LogEntry[] | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [attempt, setAttempt] = useState(0);
    const end = useRef<HTMLDivElement>(null);
    const full: LogQuery = useMemo(
        () => ({ ...query, severity: severity || null, since, text: text || null, filter: extraFilter || query.filter || null, limit: 300 }),
        [query, severity, since, text, extraFilter],
    );

    useEffect(() => {
        if (!active) return;
        setError(null);
        if (live) {
            setEntries([]);
            const stream = gcpApi.tailLogs(
                target,
                { ...full, since: "15m" },
                {
                    onEntries: (fresh) => setEntries((held) => [...(held ?? []), ...fresh].slice(-MAX_ENTRIES)),
                    onError: (message) => {
                        setError(message);
                        setLive(false);
                    },
                },
            );
            return () => stream.stop();
        }
        let current = true;
        setEntries(null);
        gcpApi
            .logs(target, full)
            .then((found) => current && setEntries([...found].reverse()))
            .catch((failure: unknown) => current && setError(errorText(failure)));
        return () => {
            current = false;
        };
    }, [active, live, target, full, attempt]);

    useEffect(() => {
        if (live) end.current?.scrollIntoView({ block: "end" });
    }, [entries, live]);

    return (
        <div className="gcp-logs">
            <div className="gcp-log-bar">
                <input
                    className="gcp-input grow"
                    value={words}
                    onChange={(event) => setWords(event.target.value)}
                    placeholder="Search logs"
                    aria-label="Search logs"
                    spellCheck={false}
                />
                <Dropdown label="Severity" value={severity} options={SEVERITIES} onChange={setSeverity} />
                <Dropdown label="Time range" value={since} options={SPANS} onChange={setSince} />
                <button type="button" className={`gcp-button${live ? " primary" : ""}`} aria-pressed={live} onClick={() => setLive(!live)}>
                    {live ? "Stop streaming" : "Stream logs"}
                </button>
            </div>
            {error ? (
                <Problem error={error} what="logs" onRetry={() => setAttempt((n) => n + 1)} />
            ) : entries === null ? (
                <SkeletonRows rows={8} label="Loading log entries" />
            ) : entries.length === 0 ? (
                <EmptyState message={live ? "Waiting for new entries…" : "No entries match."} />
            ) : (
                <>
                    <Histogram entries={entries} />
                    <div className="gcp-log-list" role="log" aria-label="Log entries">
                        {entries.map((entry, index) => (
                            <Entry key={`${entry.id}-${index}`} entry={entry} />
                        ))}
                        <div ref={end} />
                    </div>
                </>
            )}
        </div>
    );
}
