import { useEffect, useRef, useState } from "react";
import { swallow } from "../../../plugin-api/host";
import { rundeckApi, type LogEntry, type LogTick, type RundeckExecution, type RundeckWorkflowState } from "../api";

export const MAX_LOG_ENTRIES = 10_000;

export interface LogRow extends LogEntry {
    /** Arrival order; stays with the line when older lines are trimmed away. */
    seq: number;
}

export interface LogState {
    rows: LogRow[];
    dropped: number;
    completed: boolean;
    failed: boolean;
    error: string | null;
}

const EMPTY_LOGS: LogState = { rows: [], dropped: 0, completed: false, failed: false, error: null };

export function appendLogTick(prev: LogState, tick: LogTick, firstSeq: number): LogState {
    let rows = prev.rows;
    let dropped = prev.dropped;
    if (tick.entries.length) {
        rows = rows.concat(tick.entries.map((entry, i) => ({ ...entry, seq: firstSeq + i })));
        if (rows.length > MAX_LOG_ENTRIES) {
            dropped += rows.length - MAX_LOG_ENTRIES;
            rows = rows.slice(-MAX_LOG_ENTRIES);
        }
    }
    return {
        rows,
        dropped,
        completed: prev.completed || tick.completed,
        failed: tick.failed,
        error: tick.error,
    };
}

/**
 * Follows one execution's status and log output while the pane is showing.
 * Hiding the window stops both streams; showing it again resumes the log from
 * where it stopped, keeping the lines already read.
 */
export function useExecutionStreams(executionId: number, active: boolean) {
    const [execution, setExecution] = useState<RundeckExecution | null>(null);
    const [state, setState] = useState<RundeckWorkflowState | null>(null);
    const [terminal, setTerminal] = useState(false);
    const [watchErr, setWatchErr] = useState<string | null>(null);
    const [logs, setLogs] = useState<LogState>(EMPTY_LOGS);
    const offset = useRef<string | null>(null);
    const seq = useRef(0);
    const logsDone = useRef(false);
    const watchDone = useRef(false);

    useEffect(() => {
        if (!active) return;
        let watchId: number | undefined;
        let logsId: number | undefined;
        let watchStarting = false;
        let logsStarting = false;
        let generation = 0;
        let alive = true;

        const startWatch = () => {
            if (watchId != null || watchStarting || watchDone.current || document.hidden || !alive) return;
            const startedIn = generation;
            watchStarting = true;
            rundeckApi
                .watchStart(executionId, (u) => {
                    if (!alive || startedIn !== generation) return;
                    if (u.execution) setExecution(u.execution);
                    if (u.state) setState(u.state);
                    setTerminal(u.terminal);
                    if (u.terminal) watchDone.current = true;
                    setWatchErr(u.error);
                })
                .then((id) => {
                    if (!alive || startedIn !== generation) void rundeckApi.watchStop(id);
                    else watchId = id;
                })
                .catch((e) => {
                    if (alive && startedIn === generation) setWatchErr(String(e));
                })
                .finally(() => {
                    watchStarting = false;
                    if (startedIn !== generation) startWatch();
                });
        };

        const startLogs = () => {
            if (logsId != null || logsStarting || logsDone.current || document.hidden || !alive) return;
            const startedIn = generation;
            logsStarting = true;
            rundeckApi
                .logsStart(executionId, offset.current, null, (tick) => {
                    if (!alive || startedIn !== generation) return;
                    const first = seq.current;
                    seq.current += tick.entries.length;
                    offset.current = tick.offset;
                    if (tick.completed) logsDone.current = true;
                    setLogs((prev) => appendLogTick(prev, tick, first));
                })
                .then((id) => {
                    if (!alive || startedIn !== generation) void rundeckApi.logsStop(id);
                    else logsId = id;
                })
                .catch((error) => {
                    if (alive && startedIn === generation) swallow("rnd logs start")(error);
                })
                .finally(() => {
                    logsStarting = false;
                    if (startedIn !== generation) startLogs();
                });
        };

        const start = () => {
            startWatch();
            startLogs();
        };

        const stop = () => {
            generation += 1;
            if (watchId != null) {
                void rundeckApi.watchStop(watchId);
                watchId = undefined;
            }
            if (logsId != null) {
                void rundeckApi.logsStop(logsId);
                logsId = undefined;
            }
        };

        if (!document.hidden) start();
        const onVisibility = () => {
            if (document.hidden) stop();
            else start();
        };
        document.addEventListener("visibilitychange", onVisibility);

        return () => {
            alive = false;
            document.removeEventListener("visibilitychange", onVisibility);
            stop();
        };
    }, [executionId, active]);

    return { execution, state, terminal, watchErr, logs };
}
