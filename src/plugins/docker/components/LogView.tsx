import { errorText } from "../../../plugin-api/format";
import { useEffect, useMemo, useRef, useState } from "react";
import { copyWithNotice } from "../../../plugin-api/host";
import { SendToAgentMenu, VirtualLogList } from "../../../plugin-api/ui";
import { dockerApi, type ContainerDetail, type LogLine } from "../api";
import { containerForAgent } from "../forAgent";
import { appendLines, clockOf, filterLines, linesAsText } from "../logs";

/** How close to the bottom counts as reading the newest lines, so the log keeps following. */
const FOLLOW_SLACK_PX = 40;

/**
 * The container's logs, followed as it writes them. A container that starts
 * again is followed again, since the engine ends a log when its container stops.
 */
export function LogView({ container }: { container: ContainerDetail }) {
    const [lines, setLines] = useState<LogLine[]>([]);
    const [ended, setEnded] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [query, setQuery] = useState("");
    const [errorsOnly, setErrorsOnly] = useState(false);
    const [withTime, setWithTime] = useState(false);
    const [wrap, setWrap] = useState(true);
    const [agentMenu, setAgentMenu] = useState<{ x: number; y: number } | null>(null);
    const atBottom = useRef(true);
    const run = `${container.id}:${container.state.startedAt ?? ""}`;

    useEffect(() => {
        setLines([]);
        setEnded(false);
        setError(null);
        const stream = dockerApi.followLogs(container.id, {
            onItem: (batch) => {
                setLines((before) => appendLines(before, batch.lines));
                if (batch.ended) setEnded(true);
            },
            onEnd: () => setEnded(true),
            onError: (failure) => setError(errorText(failure)),
        });
        return () => stream.stop();
        // eslint-disable-next-line react-hooks/exhaustive-deps -- the run stands in for the container's id and when it last started
    }, [run]);

    const shown = useMemo(() => filterLines(lines, { query, errorsOnly }), [lines, query, errorsOnly]);
    const filtered = shown.length !== lines.length;

    return (
        <div className="docker-logs">
            <div className="docker-logs-bar">
                <input
                    className="docker-find docker-logs-find"
                    value={query}
                    onChange={(event) => setQuery(event.target.value)}
                    onKeyDown={(event) => {
                        if (event.key === "Escape") setQuery("");
                    }}
                    placeholder="Search the log…"
                    aria-label="Search the log"
                    spellCheck={false}
                />
                <Toggle label="Errors only" on={errorsOnly} onChange={setErrorsOnly} />
                <Toggle label="Times" on={withTime} onChange={setWithTime} />
                <Toggle label="Wrap" on={wrap} onChange={setWrap} />
                <span className="docker-muted docker-logs-count" role="status">
                    {filtered ? `${shown.length} of ${lines.length} lines` : `${lines.length} lines`}
                </span>
                <button
                    type="button"
                    className="docker-button"
                    disabled={shown.length === 0}
                    onClick={() => copyWithNotice(linesAsText(shown, withTime), filtered ? "Matching log lines" : "Log")}>
                    Copy
                </button>
                <button type="button" className="docker-button" disabled={lines.length === 0} onClick={() => setLines([])}>
                    Clear
                </button>
                <button
                    type="button"
                    className="docker-button"
                    onClick={(event) => {
                        const box = event.currentTarget.getBoundingClientRect();
                        setAgentMenu({ x: box.right, y: box.bottom + 4 });
                    }}>
                    Ask an agent
                </button>
            </div>
            {agentMenu ? (
                <SendToAgentMenu
                    x={agentMenu.x}
                    y={agentMenu.y}
                    alignRight
                    delivery={() => ({ text: containerForAgent(container, shown) })}
                    onClose={() => setAgentMenu(null)}
                />
            ) : null}
            {error ? <p className="docker-note">{error}</p> : null}
            <VirtualLogList
                items={shown}
                className={`docker-log-list${wrap ? " wrap" : ""}`}
                rowClassName={(line) => `docker-log-line ${line.stream}`}
                estimateSize={18}
                follow
                allowFollow={() => atBottom.current}
                onScroll={(element) => {
                    atBottom.current = element.scrollHeight - element.scrollTop - element.clientHeight < FOLLOW_SLACK_PX;
                }}
                getItemKey={(_, index) => index}
                empty={
                    <p className="docker-note docker-log-empty">
                        {lines.length === 0 ? (ended ? "Nothing logged." : "Waiting for output…") : "No lines match."}
                    </p>
                }
                renderRow={(line) => (
                    <>
                        {withTime ? <span className="docker-log-time">{clockOf(line.at)}</span> : null}
                        <span className="docker-log-text">{line.text || " "}</span>
                    </>
                )}
            />
            {ended && !container.state.running ? (
                <p className="docker-muted docker-log-ended">The log ended when the container stopped. It picks up again when it starts.</p>
            ) : null}
        </div>
    );
}

function Toggle({ label, on, onChange }: { label: string; on: boolean; onChange: (on: boolean) => void }) {
    return (
        <button type="button" className={`docker-toggle${on ? " on" : ""}`} aria-pressed={on} onClick={() => onChange(!on)}>
            {label}
        </button>
    );
}
