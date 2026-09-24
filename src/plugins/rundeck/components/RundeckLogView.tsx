import { useMemo, useState } from "react";
import { openUrl, swallow } from "../../../plugin-api/host";
import { Dropdown, EmptyState, Switch, VirtualLogList } from "../../../plugin-api/ui";
import { AnsiText } from "./AnsiText";
import type { LogRow, LogState } from "./useExecutionStreams";

type LevelFilter = "all" | "no-debug";

const LEVEL_OPTIONS = [
    { value: "all", label: "all levels" },
    { value: "no-debug", label: "hide debug" },
];

export function matchesStep(stepctx: string | null, key: string): boolean {
    const ctx = stepctx ?? "";
    return ctx === key || ctx.startsWith(`${key}/`) || ctx.startsWith(`${key}@`);
}

interface Props {
    logs: LogState;
    stepFilter: string | null;
    stepLabel: string | null;
    terminal: boolean;
    permalink: string | null;
}

export function RundeckLogView({ logs, stepFilter, stepLabel, terminal, permalink }: Props) {
    const [followTail, setFollowTail] = useState(true);
    const [levelFilter, setLevelFilter] = useState<LevelFilter>("all");

    const rows = useMemo(() => {
        if (!stepFilter && levelFilter === "all") return logs.rows;
        return logs.rows.filter(
            (row) => (!stepFilter || matchesStep(row.stepctx, stepFilter)) && (levelFilter === "all" || (row.level ?? "").toUpperCase() !== "DEBUG"),
        );
    }, [logs.rows, stepFilter, levelFilter]);

    const multiNode = useMemo(() => {
        let first: string | null = null;
        for (const row of logs.rows) {
            if (!row.node) continue;
            if (first === null) first = row.node;
            else if (row.node !== first) return true;
        }
        return false;
    }, [logs.rows]);

    const streamState = logs.completed ? "ended" : logs.failed ? "stopped" : terminal ? "ending…" : "live";

    return (
        <section className="rnd-logs">
            <div className="rnd-logs-toolbar">
                <span className="rnd-logs-title">
                    output · {stepLabel ?? "all steps"} · {streamState}
                </span>
                <Dropdown
                    className="rnd-logs-level"
                    value={levelFilter}
                    options={LEVEL_OPTIONS}
                    onChange={(v) => setLevelFilter(v as LevelFilter)}
                    label="Log level filter"
                />
                <label className="rnd-toggle">
                    <span>follow</span>
                    <Switch checked={followTail} onChange={setFollowTail} label="Follow log tail" />
                </label>
                <span className="rnd-logs-count">{rows.length} lines</span>
            </div>
            {logs.failed && <div className="rnd-banner danger">log stream stopped: {logs.error ?? "too many errors"}</div>}
            {!logs.failed && logs.error && <div className="rnd-banner warn">log stream: {logs.error}</div>}
            {logs.dropped > 0 && (
                <div className="rnd-banner muted rnd-logs-dropped">
                    {logs.dropped.toLocaleString()} earlier lines not shown
                    {permalink && (
                        <>
                            {" · "}
                            <button type="button" className="rnd-link" onClick={() => void openUrl(permalink).catch(swallow("open Rundeck URL"))}>
                                open in Rundeck
                            </button>
                        </>
                    )}
                </div>
            )}
            <VirtualLogList
                items={rows}
                className={`rnd-logs-stream${multiNode ? " multi-node" : ""}`}
                rowClassName={(row) => `rnd-log-line${row.level ? ` lvl-${row.level.toLowerCase()}` : ""}`}
                estimateSize={20}
                follow={followTail}
                empty={<EmptyState message={`no output${stepFilter ? " for this step" : ""} yet`} />}
                getItemKey={(row) => row.seq}
                renderRow={(row: LogRow) => (
                    <>
                        <span className="rnd-log-step">{row.stepctx ? `[${row.stepctx}]` : ""}</span>
                        {multiNode && <span className="rnd-log-node">{row.node ?? ""}</span>}
                        <AnsiText className="rnd-log-text" text={row.log ?? ""} />
                    </>
                )}
            />
        </section>
    );
}
