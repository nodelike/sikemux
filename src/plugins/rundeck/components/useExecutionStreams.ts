import { useRef, useState } from "react";
import { errorText } from "../../../plugin-api/format";
import { useVisibleStream } from "../../../plugin-api/hooks";
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

    useVisibleStream(
        active && !terminal
            ? (life) =>
                  rundeckApi.watch(executionId, {
                      onItem: (update) => {
                          if (update.execution) setExecution(update.execution);
                          if (update.state) setState(update.state);
                          setTerminal(update.terminal);
                          setWatchErr(update.error);
                          if (update.terminal) life.finish();
                      },
                      onError: (error) => setWatchErr(errorText(error)),
                  })
            : null,
        [executionId],
        { pauseWhenHidden: true },
    );

    useVisibleStream(
        active && !logs.completed
            ? (life) =>
                  rundeckApi.logs(executionId, offset.current, {
                      onItem: (tick) => {
                          const first = seq.current;
                          seq.current += tick.entries.length;
                          offset.current = tick.offset;
                          setLogs((prev) => appendLogTick(prev, tick, first));
                          if (tick.completed) life.finish();
                      },
                      onError: swallow("rnd logs start"),
                  })
            : null,
        [executionId],
        { pauseWhenHidden: true },
    );

    return { execution, state, terminal, watchErr, logs };
}
