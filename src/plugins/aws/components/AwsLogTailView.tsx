import { useEffect, useState } from "react";
import { reportError } from "../../../plugin-api/host";
import { VirtualLogList } from "../../../plugin-api/ui";
import { awsApi } from "../api";
import { highlightLog } from "./logHighlight";

const MAX_LINES = 5000;
const FLUSH_MS = 50;

interface Props {
    profile: string;
    logGroup: string;
    logStream?: string | null;
    active: boolean;
}

export function AwsLogTailView({ profile, logGroup, logStream, active }: Props) {
    const [lines, setLines] = useState<string[]>([]);
    const [err, setErr] = useState<string | null>(null);
    const [live, setLive] = useState(false);
    const [pinned, setPinned] = useState(true);

    useEffect(() => {
        if (!active) return;
        let cancelled = false;
        let flushTimer: number | undefined;
        const pending: string[] = [];
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
                    pending.push(line);
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

    const onScroll = (el: HTMLDivElement) => {
        const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 20;
        setPinned(atBottom);
    };

    if (err) return <div className="aws-err">{err}</div>;

    return (
        <div className="aws-logs">
            <div className="aws-logs-head">
                <span className="aws-logs-target">
                    <span className="aws-logs-label">group</span>
                    <span className="aws-logs-value">{logGroup}</span>
                </span>
                {logStream && (
                    <span className="aws-logs-target">
                        <span className="aws-logs-label">stream</span>
                        <span className="aws-logs-value">{logStream}</span>
                    </span>
                )}
                <span className={`aws-logs-pill ${live ? "live" : "ended"}`}>
                    <span className="aws-logs-pill-dot" />
                    {live ? "tailing" : "ended"}
                </span>
                {!pinned && (
                    <button
                        className="aws-logs-jump"
                        onClick={() => {
                            setPinned(true);
                        }}>
                        ↓ jump to live
                    </button>
                )}
            </div>

            <VirtualLogList
                items={lines}
                className="aws-logs-body"
                rowClassName="aws-logs-line"
                estimateSize={19}
                follow={pinned}
                onScroll={onScroll}
                allowFollow={(el) => {
                    const sel = window.getSelection();
                    return !(sel && sel.toString() && el.contains(sel.anchorNode));
                }}
                empty={<div className="aws-logs-waiting">no events in the last 5 minutes — waiting for new ones…</div>}
                renderRow={(line) => highlightLog(line)}
            />
        </div>
    );
}
