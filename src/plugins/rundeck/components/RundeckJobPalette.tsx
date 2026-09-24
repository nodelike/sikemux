import { useEffect, useMemo, useRef, useState } from "react";
import type { RundeckJob } from "../api";
import { IconCommand, IconSearch, rankBy, useMouseActive } from "../../../plugin-api/ui";
import * as cmd from "../state";
import { useResource } from "../../../plugin-api/resources";
import { rndJobIndexR } from "../resources";
import { groupSegments, targetTone } from "../shape";
import { RUNDECK_DEPLOY } from "../kinds";
import { useActiveProjectCwd, useActiveSurfacePane } from "../../../plugin-api/host";
import "../rundeck.css";

const MAX_RESULTS = 400;

export function RundeckJobPalette() {
    const paneId = useActiveSurfacePane(RUNDECK_DEPLOY);
    const activeCwd = useActiveProjectCwd();
    const prodEnvs = cmd.rundeckSettings.useSelect((s) => s.prodEnvs);

    const [query, setQuery] = useState("");
    const [sel, setSel] = useState(0);
    const inputRef = useRef<HTMLInputElement>(null);
    const listRef = useRef<HTMLDivElement>(null);
    const mouseActive = useMouseActive();

    const index = useResource(rndJobIndexR);

    useEffect(() => {
        inputRef.current?.focus();
    }, []);

    const all = useMemo<RundeckJob[]>(() => {
        const rows = (index.data ?? []).flatMap((entry) => entry.jobs);
        return rows.sort(
            (a, b) => a.project.localeCompare(b.project) || (a.group ?? "").localeCompare(b.group ?? "") || a.name.localeCompare(b.name),
        );
    }, [index.data]);

    const failedProjects = (index.data ?? []).filter((entry) => entry.error).length;

    const items = useMemo(
        () =>
            rankBy(query, all, (job) => [
                job.name,
                `${job.group ?? ""}/${job.name}`,
                `${job.project} ${job.group ?? ""} ${job.name}`.toLowerCase(),
            ]).slice(0, MAX_RESULTS),
        [all, query],
    );

    useEffect(() => {
        setSel((s) => Math.min(s, Math.max(0, items.length - 1)));
    }, [items.length]);

    useEffect(() => {
        const el = listRef.current?.querySelector<HTMLElement>(`.picker-item:nth-child(${sel + 1})`);
        el?.scrollIntoView({ block: "nearest" });
    }, [sel]);

    const activate = (job: RundeckJob | undefined) => {
        if (!job) return;
        const repoPath = cmd.linkedRepoPath({ jobId: job.id, name: job.name }, activeCwd);
        cmd.closeRundeckJobPalette();
        cmd.openRundeckJob({ project: job.project, jobId: job.id, name: job.name, group: job.group, repoPath }, { paneId, push: true });
    };

    const onKeyDown = (e: React.KeyboardEvent) => {
        if (e.key === "Escape") {
            cmd.closeRundeckJobPalette();
        } else if (e.key === "ArrowDown" || (e.key === "Tab" && !e.shiftKey)) {
            e.preventDefault();
            setSel((s) => (items.length ? (s + 1) % items.length : 0));
        } else if (e.key === "ArrowUp" || (e.key === "Tab" && e.shiftKey)) {
            e.preventDefault();
            setSel((s) => (items.length ? (s - 1 + items.length) % items.length : 0));
        } else if (e.key === "Enter") {
            e.preventDefault();
            activate(items[sel]);
        }
    };

    const loading = index.status === "loading" && all.length === 0;

    return (
        <div className="picker-backdrop" onMouseDown={cmd.closeRundeckJobPalette}>
            <div className="picker" onMouseDown={(e) => e.stopPropagation()}>
                <div className="picker-input-wrap">
                    <IconSearch size={15} className="picker-search-icon" />
                    <input
                        ref={inputRef}
                        className="picker-input"
                        placeholder="search Rundeck jobs..."
                        value={query}
                        onChange={(e) => {
                            setQuery(e.target.value);
                            setSel(0);
                        }}
                        onKeyDown={onKeyDown}
                        spellCheck={false}
                    />
                </div>

                <div className="picker-list" ref={listRef}>
                    {items.length === 0 && (
                        <div className="picker-empty">{loading ? "loading jobs..." : index.error ? index.error : "no matches"}</div>
                    )}
                    {items.map((job, i) => (
                        <button
                            key={`${job.project}:${job.id}`}
                            className={`picker-item${i === sel ? " sel" : ""}`}
                            onMouseEnter={() => {
                                if (mouseActive.current) setSel(i);
                            }}
                            onClick={() => activate(job)}>
                            <span className="picker-icon command">
                                <IconCommand size={14} />
                            </span>
                            <span className="picker-name">{job.name}</span>
                            <JobTags job={job} prodEnvs={prodEnvs} />
                        </button>
                    ))}
                </div>
                {failedProjects > 0 && (
                    <div className="picker-empty">
                        {failedProjects} project{failedProjects === 1 ? "" : "s"} couldn't be listed
                    </div>
                )}
            </div>
        </div>
    );
}

function JobTags({ job, prodEnvs }: { job: RundeckJob; prodEnvs: string[] }) {
    const group = groupSegments(job.group).join("/");
    const tone = targetTone(job.project, job.group, prodEnvs);
    return (
        <span className="picker-tags">
            <span className="picker-tag proj">{job.project}</span>
            {group && (
                <span className={`picker-tag env env-${tone}`}>
                    <span className="picker-tag-dot" />
                    {group}
                </span>
            )}
        </span>
    );
}
