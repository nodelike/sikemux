import { useMemo, useState } from "react";
import { openUrl, swallow } from "../../../plugin-api/host";
import { Switch, VirtualLogList } from "../../../plugin-api/ui";
import { AnsiText } from "./AnsiText";
import type { LogRow, LogState } from "./useExecutionStreams";
import { Seg } from "./parts";

type LevelFilter = "all" | "no-debug" | "errors";

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
        return logs.rows.filter((row) => {
            if (stepFilter && !matchesStep(row.stepctx, stepFilter)) return false;
            const level = (row.level ?? "").toUpperCase();
            if (levelFilter === "no-debug") return level !== "DEBUG" && level !== "VERBOSE";
            if (levelFilter === "errors") return level === "ERROR" || level === "WARN";
            return true;
        });
    }, [logs.rows, stepFilter, levelFilter]);

    const errors = useMemo(() => logs.rows.filter((row) => (row.level ?? "").toUpperCase() === "ERROR").length, [logs.rows]);

    const multiNode = useMemo(() => {
        let first: string | null = null;
        for (const row of logs.rows) {
            if (!row.node) continue;
            if (first === null) first = row.node;
            else if (row.node !== first) return true;
        }
        return false;
    }, [logs.rows]);

    const streamState = logs.completed ? "Ended" : logs.failed ? "Stopped" : terminal ? "Ending…" : "Live";

    return (
        <section className="rnd-logs">
            <div className="rnd-logs-bar">
                <span className="rnd-logs-title">{stepLabel ?? "All steps"}</span>
                <span className="rnd-grow" />
                <Seg
                    label="Log level"
                    value={levelFilter}
                    onChange={setLevelFilter}
                    options={[
                        { value: "all", label: "All" },
                        { value: "no-debug", label: "Hide debug" },
                        { value: "errors", label: "Problems", count: errors || undefined },
                    ]}
                />
                <label className="rnd-follow">
                    <span>Follow</span>
                    <Switch checked={followTail} onChange={setFollowTail} label="Follow the newest output" />
                </label>
                <span className={`rnd-live${streamState === "Live" ? "" : " ended"}`}>
                    <i />
                    {streamState}
                </span>
            </div>
            {logs.failed && <div className="rnd-banner danger rnd-inset">The log stream stopped: {logs.error ?? "too many errors"}</div>}
            {!logs.failed && logs.error && <div className="rnd-banner warn rnd-inset">Log stream: {logs.error}</div>}
            {logs.dropped > 0 && (
                <div className="rnd-banner muted rnd-inset">
                    {logs.dropped.toLocaleString()} earlier lines aren't shown
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
                empty={<div className="rnd-logs-empty">{`No output${stepFilter ? " for this step" : ""} yet.`}</div>}
                getItemKey={(row) => row.seq}
                renderRow={(row: LogRow) => (
                    <>
                        <span className="rnd-log-time">{row.time ?? ""}</span>
                        <span className="rnd-log-step">{row.stepctx ? row.stepctx.split(/[/@]/)[0] : ""}</span>
                        {multiNode && <span className="rnd-log-node">{row.node ?? ""}</span>}
                        <AnsiText className="rnd-log-text" text={row.log ?? ""} />
                    </>
                )}
            />
            <div className="rnd-logs-foot">
                <span>
                    {rows.length === logs.rows.length ? `${logs.rows.length} lines` : `${rows.length} of ${logs.rows.length} lines`} · {errors}{" "}
                    {errors === 1 ? "error" : "errors"}
                </span>
                <span>{followTail ? "Following the newest output" : "Paused"}</span>
            </div>
        </section>
    );
}
