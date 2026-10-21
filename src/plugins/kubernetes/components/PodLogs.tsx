import { useEffect, useMemo, useState } from "react";
import { VirtualLogList } from "../../../plugin-api/ui";
import { errorText } from "../../../plugin-api/format";
import { kubeApi, type LogLine, type Scope } from "../api";
import { Toggle } from "./parts";

/** The most lines kept while following, so a chatty pod can't grow the pane without end. */
const KEEP_LINES = 5_000;

export function levelOf(text: string): "error" | "warn" | null {
    if (/\b(level=error|ERROR|FATAL|panic:|Exception|Traceback)\b/.test(text) || /^panic:/.test(text)) return "error";
    if (/\b(level=warn(ing)?|WARN(ING)?)\b/.test(text)) return "warn";
    return null;
}

function time(at: string | null): string {
    return at ? at.slice(11, 19) : "";
}

export function PodLogs({
    scope,
    pod,
    containers,
    restarted,
}: {
    scope: Scope;
    pod: string;
    containers: string[];
    /** Whether any container has restarted, so its previous run's logs exist. */
    restarted: boolean;
}) {
    const [container, setContainer] = useState<string | null>(containers[0] ?? null);
    const [previous, setPrevious] = useState(false);
    const [follow, setFollow] = useState(true);
    const [wrap, setWrap] = useState(false);
    const [find, setFind] = useState("");
    const [lines, setLines] = useState<LogLine[] | null>(null);
    const [error, setError] = useState<string | null>(null);
    const [ended, setEnded] = useState(false);
    const live = follow && !previous;
    const { context, namespace } = scope;

    useEffect(() => {
        setLines(null);
        setError(null);
        setEnded(false);
        const scope = { context, namespace };
        if (live) {
            const stream = kubeApi.followLogs(scope, pod, container, {
                onItem: (batch) => {
                    setLines((held) => [...(held ?? []), ...batch.lines].slice(-KEEP_LINES));
                    if (batch.ended) setEnded(true);
                },
                onError: (failure) => setError(errorText(failure)),
            });
            return () => stream.stop();
        }
        let current = true;
        kubeApi.logs(scope, pod, container, previous).then(
            (read) => current && setLines(read.lines),
            (failure: unknown) => current && setError(errorText(failure)),
        );
        return () => {
            current = false;
        };
    }, [context, namespace, pod, container, previous, live]);

    const shown = useMemo(() => {
        const words = find.trim().toLowerCase();
        return words ? (lines ?? []).filter((line) => line.text.toLowerCase().includes(words)) : (lines ?? []);
    }, [lines, find]);

    return (
        <section className="k8s-logs" aria-label="Logs">
            <div className="k8s-toolbar">
                {containers.length > 1 ? (
                    <select
                        className="k8s-select"
                        aria-label="Container"
                        value={container ?? ""}
                        onChange={(event) => setContainer(event.target.value)}>
                        {containers.map((name) => (
                            <option key={name} value={name}>
                                {name}
                            </option>
                        ))}
                    </select>
                ) : null}
                <input
                    className="k8s-find k8s-find-inline"
                    value={find}
                    onChange={(event) => setFind(event.target.value)}
                    placeholder="Filter lines…"
                    aria-label="Filter lines"
                    spellCheck={false}
                />
                <Toggle label="Follow" checked={follow} disabled={previous} onChange={setFollow} />
                {restarted ? <Toggle label="Before the last restart" checked={previous} onChange={setPrevious} /> : null}
                <Toggle label="Wrap" checked={wrap} onChange={setWrap} />
                <span className="k8s-muted k8s-logs-count">
                    {live && !ended && !error ? <span className="k8s-live">Live</span> : null}
                    {lines ? `${shown.length.toLocaleString()} lines` : ""}
                </span>
            </div>
            {error ? (
                <p className="k8s-note k8s-error">{error}</p>
            ) : lines === null ? (
                <p className="k8s-note">Reading logs…</p>
            ) : (
                <VirtualLogList
                    items={shown}
                    className={`k8s-log-list${wrap ? " wrap" : ""}`}
                    rowClassName={(line) => `k8s-log-line ${levelOf(line.text) ?? ""}`.trim()}
                    follow={live}
                    allowFollow={(element) => element.scrollHeight - element.scrollTop - element.clientHeight < 80}
                    empty={
                        <p className="k8s-note">
                            {find ? "No lines match." : previous ? "The run before the restart wrote nothing." : "Nothing logged yet."}
                        </p>
                    }
                    renderRow={(line) => (
                        <>
                            <span className="k8s-log-time">{time(line.at)}</span>
                            <span className="k8s-log-text">{line.text}</span>
                        </>
                    )}
                />
            )}
            {ended && live ? <p className="k8s-note">The container stopped; these are its last lines.</p> : null}
        </section>
    );
}
