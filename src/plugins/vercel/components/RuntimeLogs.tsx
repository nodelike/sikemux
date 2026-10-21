import { useCallback, useEffect, useMemo, useState } from "react";
import { copyWithNotice } from "../../../plugin-api/host";
import { Seg } from "../../../plugin-api/ui";
import { errorText } from "../../../plugin-api/format";
import { vercelApi, type RuntimeLogLine, type RuntimeLogs as Logs, type VercelDeployment } from "../api";

/** How often the log is read again while Live is on. */
const LIVE_EVERY_MS = 5_000;

export type LevelFilter = "all" | "warning" | "error";
export type StatusFilter = "all" | "4xx" | "5xx";

export interface RuntimeFilter {
    level: LevelFilter;
    status: StatusFilter;
    path: string;
}

/** A function's line goes with its request: when a status is asked for, it shows if its request does. */
export function keep(lines: readonly RuntimeLogLine[], filter: RuntimeFilter): RuntimeLogLine[] {
    const statusOk = (status: number | null) =>
        filter.status === "all" || (status !== null && (filter.status === "5xx" ? status >= 500 : status >= 400 && status < 500));
    const matched = new Set(lines.filter((line) => line.kind === "request" && statusOk(line.status)).map((line) => line.requestId));
    const words = filter.path.trim().toLowerCase();
    return lines.filter((line) => {
        if (filter.level === "error" && line.level !== "error") return false;
        if (filter.level === "warning" && line.level !== "error" && line.level !== "warning") return false;
        if (filter.status !== "all" && !(line.kind === "request" ? statusOk(line.status) : matched.has(line.requestId))) return false;
        return !words || `${line.path ?? ""} ${line.message}`.toLowerCase().includes(words);
    });
}

const WINDOWS = [
    { value: "5", label: "5 min" },
    { value: "15", label: "15 min" },
    { value: "30", label: "30 min" },
];
const LEVELS = [
    { value: "all" as const, label: "All" },
    { value: "warning" as const, label: "Warnings" },
    { value: "error" as const, label: "Errors" },
];
const STATUSES = [
    { value: "all" as const, label: "Any status" },
    { value: "4xx" as const, label: "4xx" },
    { value: "5xx" as const, label: "5xx" },
];

/** The deployment's recent requests and what its functions logged, newest first, as Vercel's Logs page shows them. */
export function RuntimeLogs({ team, deployment }: { team: string; deployment: VercelDeployment }) {
    const [minutes, setMinutes] = useState("15");
    const [live, setLive] = useState(false);
    const [logs, setLogs] = useState<Logs | null>(null);
    const [loading, setLoading] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [filter, setFilter] = useState<RuntimeFilter>({ level: "all", status: "all", path: "" });
    const project = deployment.projectId ?? deployment.project;

    const read = useCallback(async () => {
        setLoading(true);
        try {
            setLogs(await vercelApi.runtimeLogs(team, project, deployment.id, Number(minutes)));
            setError(null);
        } catch (failure) {
            setError(errorText(failure));
        } finally {
            setLoading(false);
        }
    }, [team, project, deployment.id, minutes]);

    useEffect(() => {
        void read();
    }, [read]);
    useEffect(() => {
        if (!live) return;
        const timer = window.setInterval(() => void read(), LIVE_EVERY_MS);
        return () => window.clearInterval(timer);
    }, [live, read]);

    const lines = useMemo(() => logs?.lines ?? [], [logs]);
    const shown = useMemo(() => keep(lines, filter), [lines, filter]);

    return (
        <section className="vercel-box vercel-logs vercel-runtime" aria-label="Runtime Logs">
            <header className="vercel-box-head">
                <span className="vercel-box-title">Runtime Logs</span>
                <span className="vercel-actions">
                    <span className="vercel-muted" role="status">
                        {loading && !logs
                            ? "Reading…"
                            : shown.length === lines.length
                              ? `${lines.length} lines`
                              : `${shown.length} of ${lines.length}`}
                    </span>
                    <Seg label="Since" options={WINDOWS} value={minutes} onChange={setMinutes} />
                    <button
                        type="button"
                        className={`vercel-button${live ? " primary" : ""}`}
                        aria-pressed={live}
                        onClick={() => setLive((was) => !was)}>
                        Live
                    </button>
                    <button
                        type="button"
                        className="vercel-button"
                        disabled={shown.length === 0}
                        onClick={() =>
                            copyWithNotice(
                                shown
                                    .map((line) =>
                                        [line.at, line.level, line.status ?? "", line.method ?? "", line.path ?? "", line.message].join(" "),
                                    )
                                    .join("\n"),
                                "Runtime logs",
                            )
                        }>
                        Copy
                    </button>
                </span>
            </header>
            <div className="vercel-runtime-filters">
                <Seg label="Level" options={LEVELS} value={filter.level} onChange={(level) => setFilter({ ...filter, level })} />
                <Seg label="Status" options={STATUSES} value={filter.status} onChange={(status) => setFilter({ ...filter, status })} />
                <input
                    className="vercel-find"
                    aria-label="Path or words"
                    placeholder="Path or words…"
                    value={filter.path}
                    onChange={(event) => setFilter({ ...filter, path: event.target.value })}
                    spellCheck={false}
                />
            </div>
            {error ? <p className="vercel-note vercel-runtime-empty">{error}</p> : null}
            {logs?.note && lines.length > 0 ? <p className="vercel-muted vercel-runtime-empty">{logs.note}</p> : null}
            <div className="vercel-log" role="log" aria-label="Runtime log">
                {shown.length === 0 ? (
                    <p className="vercel-muted vercel-runtime-empty">
                        {lines.length > 0 ? "No lines match." : (logs?.note ?? (loading ? "Reading…" : "Nothing yet."))}
                    </p>
                ) : (
                    shown.map((line, index) => (
                        <div key={`${line.at}:${line.requestId ?? ""}:${index}`} className={`vercel-log-line runtime ${line.kind} ${line.level}`}>
                            <span className="vercel-log-time">{new Date(line.at).toLocaleTimeString(undefined, { hour12: false })}</span>
                            {line.status !== null ? (
                                <span className={`vercel-log-status s${Math.floor(line.status / 100)}`}>{line.status}</span>
                            ) : null}
                            {line.path ? (
                                <span className="vercel-log-path">
                                    {line.method ? `${line.method} ` : ""}
                                    {line.path}
                                </span>
                            ) : null}
                            <span className="vercel-log-text">{line.message}</span>
                        </div>
                    ))
                )}
            </div>
        </section>
    );
}
