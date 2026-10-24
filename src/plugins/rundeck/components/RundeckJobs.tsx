import { useMemo, useRef, useState, type KeyboardEvent } from "react";
import { useActiveProjectCwd } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { EmptyState, Filter, IconRun, IconWarning, Seg, SkeletonRows } from "../../../plugin-api/ui";
import { rndJobsV } from "../resources";
import * as cmd from "../state";
import type { JobRef } from "../state";
import { duration } from "../shape";
import type { RundeckJobRow, RundeckSection } from "../types.generated";
import { useNow } from "../../../plugin-api/hooks";
import { BranchChip, Header, Status, Tag, levelCrumbs } from "./parts";
import { formatAgo } from "../../../plugin-api/format";

interface Props {
    paneId: string;
    active: boolean;
}

type StatusFilter = "all" | "running" | "failed";

/** A section with the jobs it holds, of those shown. */
interface Shown {
    section: RundeckSection;
    jobs: RundeckJobRow[];
}

export function RundeckJobs({ paneId, active }: Props) {
    const project = cmd.rundeckSettings.useSelect((s) => s.activeProject);
    const group = cmd.rundeckSettings.useSelect((s) => s.activeGroup);
    const layoutChoice = cmd.useRundeck((s) => s.layout[paneId] ?? "list");
    const activeCwd = useActiveProjectCwd();
    const [statusFilter, setStatusFilter] = useState<StatusFilter>("all");
    const [query, setQuery] = useState("");

    const res = useResourceEnabled(active && !!project, rndJobsV, paneId, project, group ?? "");
    const data = res.data;
    const jobs = useMemo(() => new Map((data?.jobs ?? []).map((job) => [job.jobId, job])), [data]);
    const anyLive = (data?.running ?? 0) > 0;
    const now = useNow(active, anyLive ? 1_000 : 30_000);
    const canCompare = (data?.compare.length ?? 0) > 0;
    const layout = canCompare ? layoutChoice : "list";

    const visible = (job: RundeckJobRow) => {
        if (query && !job.name.toLowerCase().includes(query.trim().toLowerCase())) return false;
        if (statusFilter === "running") return !!job.latest?.live;
        if (statusFilter === "failed") return job.latest?.statusKind === "failed";
        return true;
    };
    const showing = (sections: RundeckSection[]): Shown[] =>
        sections
            .map((section) => ({ section, jobs: section.jobs.flatMap((id) => jobs.get(id) ?? []).filter(visible) }))
            .filter((shown) => shown.jobs.length > 0);
    const shownList = showing(data?.list ?? []);
    const shownCompare = showing(data?.compare ?? []);
    const flat = (layout === "list" ? shownList : shownCompare).flatMap((shown) => shown.jobs);

    const jobRef = (job: RundeckJobRow): JobRef => ({
        project,
        jobId: job.jobId,
        name: job.name,
        group: job.group,
        repoPath: cmd.linkedRepoPath({ jobId: job.jobId, name: job.name }, activeCwd),
    });
    const open = (job: RundeckJobRow) => cmd.rundeckPush(paneId, { kind: "service", ...jobRef(job) });
    const openForm = (job: RundeckJobRow) => {
        if (!job.enabled) return;
        cmd.rundeckPush(paneId, { kind: "deploy", ...jobRef(job), branch: job.deployed?.branch ?? job.latest?.branch ?? undefined });
    };

    if (!project) {
        return (
            <div className="rnd-main">
                <EmptyState message="Pick a Rundeck project from the list on the left." />
            </div>
        );
    }

    const total = data?.jobs.length ?? 0;
    return (
        <div className="rnd-main">
            <Header
                paneId={paneId}
                crumbs={levelCrumbs(paneId, { kind: "matrix" }, project, group)}
                title={data?.title ?? project}
                count={total}
                aside={
                    layout === "list" ? (
                        <Seg
                            label="Filter by last run"
                            value={statusFilter}
                            onChange={setStatusFilter}
                            options={[
                                { value: "all", label: "All", count: total },
                                { value: "running", label: "Running", count: data?.running ?? 0 },
                                { value: "failed", label: "Failed", count: data?.failed ?? 0 },
                            ]}
                        />
                    ) : (
                        <span className="rnd-head-note">
                            {data?.shared} {data?.shared === 1 ? "job" : "jobs"} in more than one folder
                        </span>
                    )
                }
                tools={
                    <>
                        {canCompare && (
                            <Seg
                                label="Layout"
                                value={layout}
                                onChange={(next) => cmd.setRundeckLayout(paneId, next)}
                                options={[
                                    { value: "list", label: "List" },
                                    { value: "compare", label: "Compare folders" },
                                ]}
                            />
                        )}
                        <Filter value={query} onChange={setQuery} placeholder="Filter jobs" />
                    </>
                }
                onRefresh={() => void res.refresh()}
                refreshing={res.status === "loading"}
            />
            {data?.error && <div className="rnd-banner warn">{data.error}</div>}
            {data?.partial && (
                <div className="rnd-banner warn">
                    Rundeck was slow to answer{data.timedOut ? `: ${data.timedOut} job${data.timedOut === 1 ? "" : "s"} timed out` : ""}. Those rows
                    show no recent runs; refresh to try again.
                </div>
            )}
            {res.error && !data && (
                <EmptyState
                    tone="error"
                    icon={<IconWarning size={14} />}
                    title="Couldn't load jobs"
                    message={res.error}
                    action={{ label: "Retry", onClick: () => void res.refresh() }}
                />
            )}
            {!data && !res.error && (
                <div className="rnd-pad">
                    <SkeletonRows rows={6} label="Loading jobs" />
                </div>
            )}
            {data && total === 0 && <EmptyState message={group ? `No jobs in ${group}.` : `No jobs in ${project}.`} />}
            {data && total > 0 && flat.length === 0 && <div className="rnd-note">No jobs match.</div>}
            {layout === "list" && flat.length > 0 && (
                <JobList paneId={paneId} project={project} sections={shownList} now={now} onOpen={open} onDeploy={openForm} />
            )}
            {layout === "compare" && flat.length > 0 && (
                <Compare sections={shownCompare} loose={data?.loose ?? 0} now={now} onOpen={open} onDeploy={openForm} />
            )}
        </div>
    );
}

function lastRunTime(job: RundeckJobRow): string | null {
    return job.latest?.endedAt ?? job.latest?.startedAt ?? null;
}

function JobList({
    paneId,
    project,
    sections,
    now,
    onOpen,
    onDeploy,
}: {
    paneId: string;
    project: string;
    sections: Shown[];
    now: number;
    onOpen: (job: RundeckJobRow) => void;
    onDeploy: (job: RundeckJobRow) => void;
}) {
    const body = useRef<HTMLTableSectionElement>(null);
    const flat = sections.flatMap((shown) => shown.jobs);
    const onKey = (event: KeyboardEvent, job: RundeckJobRow) => {
        const at = flat.indexOf(job);
        if (event.key === "Enter") {
            event.preventDefault();
            onOpen(job);
        } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            event.preventDefault();
            const next = flat[Math.max(0, Math.min(flat.length - 1, at + (event.key === "ArrowDown" ? 1 : -1)))];
            if (!next) return;
            body.current?.querySelector<HTMLElement>(`[data-job="${CSS.escape(next.jobId)}"]`)?.focus();
        }
    };
    return (
        <div className="rnd-table-wrap">
            <table className="rnd-table" aria-hidden="true">
                <JobCols />
                <thead>
                    <tr>
                        <th>Job</th>
                        <th>Branch</th>
                        <th>Last run by</th>
                        <th className="r">When</th>
                        <th className="r">Took</th>
                        <th />
                    </tr>
                </thead>
            </table>
            <div className="rnd-table-body">
                <table className="rnd-table" aria-label="Jobs">
                    <JobCols />
                    <tbody ref={body}>
                        {sections.map(({ section, jobs }) => (
                            <SectionRows
                                key={section.segment ?? "\u0000"}
                                showHead={sections.length > 1 || section.segment !== null}
                                label={section.label}
                                onOpenFolder={section.segment ? () => cmd.selectRundeckGroup(paneId, project, section.path) : undefined}
                                prod={section.prod}>
                                {jobs.map((job) => {
                                    const when = lastRunTime(job);
                                    return (
                                        <tr
                                            key={job.jobId}
                                            data-job={job.jobId}
                                            tabIndex={0}
                                            className={job.enabled ? undefined : "dim"}
                                            onClick={() => onOpen(job)}
                                            onKeyDown={(e) => onKey(e, job)}>
                                            <td>
                                                <span className="rnd-name-row">
                                                    <Status status={job.latest?.status} label="" />
                                                    <span className="rnd-name">{job.name}</span>
                                                    {job.scheduled && <Tag>scheduled</Tag>}
                                                    {!job.enabled && <Tag tone="muted">disabled</Tag>}
                                                    {job.error && <Tag tone="warn">{job.error === "timed out" ? "timed out" : "error"}</Tag>}
                                                </span>
                                            </td>
                                            <td>
                                                <BranchChip branch={job.branch} />
                                            </td>
                                            <td className="rnd-cell-sub">
                                                {job.latest ? (job.latest.user ?? "—") : <span className="rnd-dim">Never run</span>}
                                            </td>
                                            <td className="r rnd-cell-dim">{when ? formatAgo(when, now) : "—"}</td>
                                            <td className="r rnd-cell-dim">
                                                {job.latest ? duration(job.latest.startedAt, job.latest.endedAt, now) || "—" : "—"}
                                            </td>
                                            <td className="r">
                                                {job.enabled && (
                                                    <button
                                                        className="rnd-btn rnd-row-deploy"
                                                        onClick={(e) => {
                                                            e.stopPropagation();
                                                            onDeploy(job);
                                                        }}>
                                                        <IconRun size={10} />
                                                        {job.runWord}
                                                    </button>
                                                )}
                                            </td>
                                        </tr>
                                    );
                                })}
                            </SectionRows>
                        ))}
                    </tbody>
                </table>
            </div>
        </div>
    );
}

function JobCols() {
    return (
        <colgroup>
            <col style={{ width: "36%" }} />
            <col style={{ width: "24%" }} />
            <col style={{ width: "18%" }} />
            <col style={{ width: "12%" }} />
            <col style={{ width: "10%" }} />
            <col style={{ width: "96px" }} />
        </colgroup>
    );
}

function SectionRows({
    showHead,
    label,
    prod,
    onOpenFolder,
    children,
}: {
    showHead: boolean;
    label: string;
    prod: boolean;
    onOpenFolder?: () => void;
    children: React.ReactNode;
}) {
    return (
        <>
            {showHead && (
                <tr className="rnd-section-row">
                    <td colSpan={6}>
                        {onOpenFolder ? (
                            <button className={`rnd-section-link${prod ? " prod" : ""}`} onClick={onOpenFolder} title="Open this folder">
                                {label}
                            </button>
                        ) : (
                            <span className={prod ? "prod" : undefined}>{label}</span>
                        )}
                    </td>
                </tr>
            )}
            {children}
        </>
    );
}

function Compare({
    sections,
    loose,
    now,
    onOpen,
    onDeploy,
}: {
    sections: Shown[];
    loose: number;
    now: number;
    onOpen: (job: RundeckJobRow) => void;
    onDeploy: (job: RundeckJobRow) => void;
}) {
    const names = [...new Set(sections.flatMap((shown) => shown.jobs.map((job) => job.name)))].sort((a, b) => a.localeCompare(b));
    return (
        <div className="rnd-compare">
            <div className="rnd-compare-grid" style={{ gridTemplateColumns: `minmax(120px, 180px) repeat(${sections.length}, minmax(0, 1fr))` }}>
                <div className="rnd-compare-head">Job</div>
                {sections.map(({ section }) => (
                    <div key={section.path} className={`rnd-compare-head${section.prod ? " prod" : ""}`}>
                        {section.segment}
                    </div>
                ))}
                {names.map((name) => (
                    <CompareRow key={name} name={name} sections={sections} now={now} onOpen={onOpen} onDeploy={onDeploy} />
                ))}
            </div>
            {loose > 0 && (
                <div className="rnd-note">
                    {loose} {loose === 1 ? "job sits" : "jobs sit"} directly in this folder; switch to List to see {loose === 1 ? "it" : "them"}.
                </div>
            )}
        </div>
    );
}

function CompareRow({
    name,
    sections,
    now,
    onOpen,
    onDeploy,
}: {
    name: string;
    sections: Shown[];
    now: number;
    onOpen: (job: RundeckJobRow) => void;
    onDeploy: (job: RundeckJobRow) => void;
}) {
    return (
        <>
            <div className="rnd-compare-name">{name}</div>
            {sections.map(({ section, jobs }) => {
                const job = jobs.find((row) => row.name === name);
                if (!job) return <div key={section.path} className="rnd-compare-cell empty" />;
                const when = lastRunTime(job);
                return (
                    <div
                        key={section.path}
                        className="rnd-compare-cell"
                        role="button"
                        tabIndex={0}
                        onClick={() => onOpen(job)}
                        onKeyDown={(e) => {
                            if (e.key === "Enter" || e.key === " ") {
                                e.preventDefault();
                                onOpen(job);
                            }
                        }}>
                        <div className="rnd-compare-top">
                            <BranchChip branch={job.branch} />
                            {job.latest ? <Status status={job.latest.status} /> : <span className="rnd-dim">Never run</span>}
                        </div>
                        <div className="rnd-compare-foot">
                            <span className="rnd-compare-when">{job.latest ? `${job.latest.user} · ${when ? formatAgo(when, now) : ""}` : ""}</span>
                            {job.liveBranch && (
                                <span className="rnd-drift">
                                    <IconWarning size={11} />
                                    live is {job.liveBranch}
                                </span>
                            )}
                        </div>
                        {job.enabled && (
                            <button
                                className="rnd-btn rnd-row-deploy rnd-compare-deploy"
                                onClick={(e) => {
                                    e.stopPropagation();
                                    onDeploy(job);
                                }}>
                                <IconRun size={10} />
                                {job.runWord}
                            </button>
                        )}
                    </div>
                );
            })}
        </>
    );
}
