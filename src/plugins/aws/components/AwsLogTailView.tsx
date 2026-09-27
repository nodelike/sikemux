import { useEffect, useMemo, useState, type ReactNode } from "react";
import { reportError } from "../../../plugin-api/host";
import { VirtualLogList } from "../../../plugin-api/ui";
import { awsApi } from "../api";
import { highlightLog } from "./logHighlight";
import { Filter, Seg } from "./parts";

const MAX_LINES = 5000;
const FLUSH_MS = 50;
const ERROR = /\b(?:ERROR|ERR|FATAL|CRITICAL|PANIC|SEVERE)\b/i;
const WARN = /\b(?:WARN|WARNING)\b/i;
const STAMP = /^(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2}:\d{2})(?:\.\d+)?\S*\s(.*)$/s;

type Level = "all" | "warn" | "error";
type Line = { text: string; stamp: string | null; day: string | null; level: "error" | "warn" | null };

function parse(raw: string): Line {
    const stderr = raw.startsWith("[err] ");
    const m = stderr ? null : STAMP.exec(raw);
    const text = m ? m[3] : raw;
    return {
        text,
        stamp: m ? m[2] : null,
        day: m ? m[1] : null,
        level: stderr || ERROR.test(text) ? "error" : WARN.test(text) ? "warn" : null,
    };
}

interface Props {
    profile: string;
    logGroup: string;
    logStream?: string | null;
    active: boolean;
    lead?: ReactNode;
}

export function AwsLogTailView({ profile, logGroup, logStream, active, lead }: Props) {
    const [lines, setLines] = useState<Line[]>([]);
    const [err, setErr] = useState<string | null>(null);
    const [live, setLive] = useState(false);
    const [pinned, setPinned] = useState(true);
    const [level, setLevel] = useState<Level>("all");
    const [query, setQuery] = useState("");

    useEffect(() => {
        if (!active) return;
        let cancelled = false;
        let flushTimer: number | undefined;
        const pending: Line[] = [];
        setLines([]);
        setErr(null);
        setLive(true);
        const flushPending = () => {
            flushTimer = undefined;
            if (cancelled || pending.length === 0) return;
            const batch = pending.splice(0);
            setLines((prev) => {
                const next = prev.concat(batch);
                return next.length > MAX_LINES ? next.slice(-MAX_LINES) : next;
            });
        };
        const ended = () => {
            if (cancelled) return;
            flushPending();
            setLive(false);
        };
        const tail = awsApi.tailLogs(
            { profile, logGroup, logStream: logStream ?? null, since: "5m" },
            {
                onLine: (line) => {
                    if (cancelled) return;
                    if (line === "") return ended();
                    pending.push(parse(line));
                    if (flushTimer === undefined) flushTimer = window.setTimeout(flushPending, FLUSH_MS);
                },
                onEnd: ended,
                onError: (message) => {
                    if (cancelled) return;
                    setErr(message);
                    reportError("logs tail")(new Error(message));
                },
            },
        );
        return () => {
            cancelled = true;
            if (flushTimer !== undefined) window.clearTimeout(flushTimer);
            tail.stop();
            setLive(false);
        };
    }, [profile, logGroup, logStream, active]);

    const shown = useMemo(() => {
        const q = query.trim().toLowerCase();
        return lines.filter((line) => {
            if (level === "error" && line.level !== "error") return false;
            if (level === "warn" && !line.level) return false;
            return !q || line.text.toLowerCase().includes(q);
        });
    }, [lines, level, query]);
    const errors = useMemo(() => lines.filter((l) => l.level === "error").length, [lines]);
    const warnings = useMemo(() => lines.filter((l) => l.level === "warn").length, [lines]);

    const onScroll = (el: HTMLDivElement) => {
        const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 20;
        setPinned(atBottom);
    };

    return (
        <div className="aws-logs-wrap">
            <div className="aws-subbar">
                {lead}
                <div className="aws-subbar-tools">
                    <Seg
                        options={[
                            { value: "all", label: "All" },
                            { value: "warn", label: "Warnings", count: warnings + errors },
                            { value: "error", label: "Errors", count: errors },
                        ]}
                        value={level}
                        onChange={setLevel}
                    />
                    <Filter value={query} onChange={setQuery} placeholder="Filter lines" />
                </div>
            </div>
            {err ? (
                <div className="aws-note err">{err}</div>
            ) : (
                <div className="aws-logs">
                    <div className="aws-logs-bar">
                        <span>
                            group <span className="aws-mono">{logGroup}</span>
                        </span>
                        {logStream && (
                            <span className="aws-logs-stream">
                                stream <span className="aws-mono">{logStream}</span>
                            </span>
                        )}
                        <span className="aws-grow" />
                        <span>since 5 min ago</span>
                        <span className={`aws-live${live ? "" : " ended"}`}>
                            <i />
                            {live ? "Tailing" : "Ended"}
                        </span>
                    </div>
                    <VirtualLogList
                        items={shown}
                        className="aws-logs-body"
                        rowClassName={(line) => `aws-log-line${line.level === "error" ? " err" : ""}`}
                        estimateSize={20}
                        follow={pinned}
                        onScroll={onScroll}
                        allowFollow={(el) => {
                            const sel = window.getSelection();
                            return !(sel && sel.toString() && el.contains(sel.anchorNode));
                        }}
                        empty={
                            <div className="aws-logs-waiting">
                                {lines.length ? "No lines match the filter." : "No events in the last 5 minutes. Waiting for new ones…"}
                            </div>
                        }
                        renderRow={(line) => (
                            <>
                                <span className="aws-log-ts" title={line.day ? `${line.day} ${line.stamp} UTC` : undefined}>
                                    {line.stamp ?? ""}
                                </span>
                                <span className="aws-log-text">{highlightLog(line.text)}</span>
                            </>
                        )}
                    />
                    <div className="aws-logs-foot">
                        <span>
                            {lines.length} lines · {errors} errors · {warnings} warnings
                        </span>
                        {pinned ? (
                            <span>{live ? "Following · scroll up to pause" : ""}</span>
                        ) : (
                            <button className="aws-btn aws-jump" onClick={() => setPinned(true)}>
                                Jump to latest
                            </button>
                        )}
                    </div>
                </div>
            )}
        </div>
    );
}
