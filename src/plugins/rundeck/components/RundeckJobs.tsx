import { useMemo, useRef, useState, type KeyboardEvent } from "react";
import { useActiveProjectCwd } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { EmptyState, Filter, IconRun, IconWarning, Seg, SkeletonRows } from "../../../plugin-api/ui";
import type { MatrixCell } from "../api";
import { rndMatrixR } from "../resources";
import * as cmd from "../state";
import type { JobRef } from "../state";
import { childSegment, duration, groupSegments, inGroup, isLiveStatus, isProdTarget } from "../shape";
import { statusKind } from "./branchStyle";
import { useNow } from "../../../plugin-api/hooks";
import { BranchChip, Header, Status, Tag, levelCrumbs } from "./parts";
import { formatAgo } from "../../../plugin-api/format";

interface Props {
    paneId: string;
    active: boolean;
}

type StatusFilter = "all" | "running" | "failed";

interface Section {
    /** A folder under the current one, or null for jobs that sit in it directly. */
    segment: string | null;
    cells: MatrixCell[];
}

export function RundeckJobs({ paneId, active }: Props) {
    const project = cmd.rundeckSettings.useSelect((s) => s.activeProject);
    const group = cmd.rundeckSettings.useSelect((s) => s.activeGroup);
    const layoutChoice = cmd.useRundeck((s) => s.layout[paneId] ?? "list");
    const activeCwd = useActiveProjectCwd();
    const [statusFilter, setStatusFilter] = useState<StatusFilter>("all");
    const [query, setQuery] = useState("");

    const res = useResourceEnabled(active && !!project, rndMatrixR, project);
    const data = res.data;
    const cells = useMemo(
        () => (data?.cells ?? []).filter((c) => inGroup(c.group, group)).sort((a, b) => a.name.localeCompare(b.name)),
        [data, group],
    );
    const anyLive = cells.some((c) => isLiveStatus(c.latest?.status));
    const now = useNow(active, anyLive ? 1_000 : 30_000);

    const sections = useMemo(() => sectionsBy(cells, (cell) => childSegment(cell.group, group)), [cells, group]);
    const listSections = useMemo(() => sectionsBy(cells, (cell) => relativePath(cell.group, group)), [cells, group]);

    const folders = sections.filter((s) => s.segment !== null);
    const shared = useMemo(() => {
        const seen = new Map<string, number>();
        for (const section of folders) for (const name of new Set(section.cells.map((c) => c.name))) seen.set(name, (seen.get(name) ?? 0) + 1);
        return [...seen.entries()].filter(([, n]) => n > 1).length;
    }, [folders]);
    const canCompare = folders.length > 1 && shared > 0;
    const layout = canCompare ? layoutChoice : "list";

    const visible = (cell: MatrixCell) => {
        if (query && !cell.name.toLowerCase().includes(query.trim().toLowerCase())) return false;
        if (statusFilter === "running") return isLiveStatus(cell.latest?.status);
        if (statusFilter === "failed") return statusKind(cell.latest?.status) === "failed";
        return true;
    };
    const shown = sections.map((s) => ({ ...s, cells: s.cells.filter(visible) })).filter((s) => s.cells.length > 0);
    const shownList = listSections.map((s) => ({ ...s, cells: s.cells.filter(visible) })).filter((s) => s.cells.length > 0);
    const flat = (layout === "list" ? shownList : shown).flatMap((s) => s.cells);

    const jobRef = (cell: MatrixCell): JobRef => ({
        project,
        jobId: cell.job_id,
        name: cell.name,
        group: cell.group,
        repoPath: cmd.linkedRepoPath({ jobId: cell.job_id, name: cell.name }, activeCwd),
    });
    const open = (cell: MatrixCell) => cmd.rundeckPush(paneId, { kind: "service", ...jobRef(cell) });
    const openForm = (cell: MatrixCell) => {
        if (cell.enabled === false) return;
        const branch = cell.deployed?.branch ?? cell.latest?.branch;
        cmd.rundeckPush(paneId, { kind: "deploy", ...jobRef(cell), branch: branch ?? undefined });
    };

    if (!project) {
        return (
            <div className="rnd-main">
                <EmptyState message="Pick a Rundeck project from the list on the left." />
            </div>
        );
    }

    const running = cells.filter((c) => isLiveStatus(c.latest?.status)).length;
    const failed = cells.filter((c) => statusKind(c.latest?.status) === "failed").length;
    const title = groupSegments(group).pop() ?? project;
    const timedOut = cells.filter((c) => c.error === "timed out").length;

    return (
        <div className="rnd-main">
            <Header
                paneId={paneId}
                crumbs={levelCrumbs(paneId, { kind: "matrix" }, project, group)}
                title={title}
                count={cells.length}
                aside={
                    layout === "list" ? (
                        <Seg
                            label="Filter by last run"
                            value={statusFilter}
                            onChange={setStatusFilter}
                            options={[
                                { value: "all", label: "All", count: cells.length },
                                { value: "running", label: "Running", count: running },
                                { value: "failed", label: "Failed", count: failed },
                            ]}
                        />
                    ) : (
                        <span className="rnd-head-note">
                            {shared} {shared === 1 ? "job" : "jobs"} in more than one folder
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
                    Rundeck was slow to answer{timedOut ? `: ${timedOut} job${timedOut === 1 ? "" : "s"} timed out` : ""}. Those rows show no recent
                    runs; refresh to try again.
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
            {data && cells.length === 0 && <EmptyState message={group ? `No jobs in ${group}.` : `No jobs in ${project}.`} />}
            {data && cells.length > 0 && flat.length === 0 && <div className="rnd-note">No jobs match.</div>}
            {layout === "list" && flat.length > 0 && (
                <JobList paneId={paneId} project={project} group={group} sections={shownList} now={now} onOpen={open} onDeploy={openForm} />
            )}
            {layout === "compare" && flat.length > 0 && (
                <Compare
                    project={project}
                    group={group}
                    sections={shown.filter((s) => s.segment !== null)}
                    loose={shown.find((s) => s.segment === null)?.cells.length ?? 0}
                    now={now}
                    onOpen={open}
                    onDeploy={openForm}
                />
            )}
        </div>
    );
}

function sectionsBy(cells: MatrixCell[], key: (cell: MatrixCell) => string | null): Section[] {
    const map = new Map<string | null, MatrixCell[]>();
    for (const cell of cells) {
        const segment = key(cell);
        map.set(segment, [...(map.get(segment) ?? []), cell]);
    }
    return [...map.entries()]
        .sort(([a], [b]) => (a === null ? -1 : b === null ? 1 : a.localeCompare(b)))
        .map(([segment, list]) => ({ segment, cells: list }));
}

/** Where a job sits below the current folder, or null when it sits in the folder itself. */
function relativePath(jobGroup: string | null, group: string | null): string | null {
    const below = groupSegments(jobGroup).slice(groupSegments(group).length);
    return below.length ? below.join("/") : null;
}

function lastRunTime(cell: MatrixCell): string | null {
    return cell.latest?.ended_at ?? cell.latest?.started_at ?? null;
}

function JobList({
    paneId,
    project,
    group,
    sections,
    now,
    onOpen,
    onDeploy,
}: {
    paneId: string;
    project: string;
    group: string | null;
    sections: Section[];
    now: number;
    onOpen: (cell: MatrixCell) => void;
    onDeploy: (cell: MatrixCell) => void;
}) {
    const body = useRef<HTMLTableSectionElement>(null);
    const prodEnvs = cmd.rundeckSettings.useSelect((s) => s.prodEnvs);
    const flat = sections.flatMap((s) => s.cells);
    const onKey = (event: KeyboardEvent, cell: MatrixCell) => {
        const at = flat.indexOf(cell);
        if (event.key === "Enter") {
            event.preventDefault();
            onOpen(cell);
        } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            event.preventDefault();
            const next = flat[Math.max(0, Math.min(flat.length - 1, at + (event.key === "ArrowDown" ? 1 : -1)))];
            if (!next) return;
            body.current?.querySelector<HTMLElement>(`[data-job="${CSS.escape(next.job_id)}"]`)?.focus();
        }
    };
    const folderPath = (segment: string) => [...groupSegments(group), segment].join("/");
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
                        {sections.map((section) => (
                            <SectionRows
                                key={section.segment ?? "\u0000"}
                                showHead={sections.length > 1 || section.segment !== null}
                                label={section.segment ? folderPath(section.segment) : groupSegments(group).join("/") || project}
                                onOpenFolder={
                                    section.segment ? () => cmd.selectRundeckGroup(paneId, project, folderPath(section.segment!)) : undefined
                                }
                                prod={isProdTarget(project, section.segment ? folderPath(section.segment) : group, prodEnvs)}>
                                {section.cells.map((cell) => {
                                    const when = lastRunTime(cell);
                                    const disabled = cell.enabled === false;
                                    return (
                                        <tr
                                            key={cell.job_id}
                                            data-job={cell.job_id}
                                            tabIndex={0}
                                            className={disabled ? "dim" : undefined}
                                            onClick={() => onOpen(cell)}
                                            onKeyDown={(e) => onKey(e, cell)}>
                                            <td>
                                                <span className="rnd-name-row">
                                                    <Status status={cell.latest?.status} label="" />
                                                    <span className="rnd-name">{cell.name}</span>
                                                    {cell.scheduled && <Tag>scheduled</Tag>}
                                                    {disabled && <Tag tone="muted">disabled</Tag>}
                                                    {cell.error && <Tag tone="warn">{cell.error === "timed out" ? "timed out" : "error"}</Tag>}
                                                </span>
                                            </td>
                                            <td>
                                                <BranchChip branch={cell.latest?.branch ?? cell.deployed?.branch} />
                                            </td>
                                            <td className="rnd-cell-sub">
                                                {cell.latest ? (cell.latest.user ?? "—") : <span className="rnd-dim">Never run</span>}
                                            </td>
                                            <td className="r rnd-cell-dim">{when ? formatAgo(when, now) : "—"}</td>
                                            <td className="r rnd-cell-dim">
                                                {cell.latest ? duration(cell.latest.started_at, cell.latest.ended_at, now) || "—" : "—"}
                                            </td>
                                            <td className="r">
                                                {!disabled && (
                                                    <button
                                                        className="rnd-btn rnd-row-deploy"
                                                        onClick={(e) => {
                                                            e.stopPropagation();
                                                            onDeploy(cell);
                                                        }}>
                                                        <IconRun size={10} />
                                                        {cell.latest?.branch || cell.deployed?.branch ? "Deploy" : "Run"}
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
    project,
    group,
    sections,
    loose,
    now,
    onOpen,
    onDeploy,
}: {
    project: string;
    group: string | null;
    sections: Section[];
    loose: number;
    now: number;
    onOpen: (cell: MatrixCell) => void;
    onDeploy: (cell: MatrixCell) => void;
}) {
    const prodEnvs = cmd.rundeckSettings.useSelect((s) => s.prodEnvs);
    const names = [...new Set(sections.flatMap((s) => s.cells.map((c) => c.name)))].sort((a, b) => a.localeCompare(b));
    const path = (segment: string) => [...groupSegments(group), segment].join("/");
    return (
        <div className="rnd-compare">
            <div className="rnd-compare-grid" style={{ gridTemplateColumns: `minmax(120px, 180px) repeat(${sections.length}, minmax(0, 1fr))` }}>
                <div className="rnd-compare-head">Job</div>
                {sections.map((s) => (
                    <div key={s.segment} className={`rnd-compare-head${isProdTarget(project, path(s.segment!), prodEnvs) ? " prod" : ""}`}>
                        {s.segment}
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
    sections: Section[];
    now: number;
    onOpen: (cell: MatrixCell) => void;
    onDeploy: (cell: MatrixCell) => void;
}) {
    return (
        <>
            <div className="rnd-compare-name">{name}</div>
            {sections.map((section) => {
                const cell = section.cells.find((c) => c.name === name);
                if (!cell) return <div key={section.segment} className="rnd-compare-cell empty" />;
                const when = lastRunTime(cell);
                const live = isLiveStatus(cell.latest?.status);
                const drift = live && cell.deployed?.branch && cell.latest?.branch && cell.deployed.branch !== cell.latest.branch;
                return (
                    <div
                        key={section.segment}
                        className="rnd-compare-cell"
                        role="button"
                        tabIndex={0}
                        onClick={() => onOpen(cell)}
                        onKeyDown={(e) => {
                            if (e.key === "Enter" || e.key === " ") {
                                e.preventDefault();
                                onOpen(cell);
                            }
                        }}>
                        <div className="rnd-compare-top">
                            <BranchChip branch={cell.latest?.branch ?? cell.deployed?.branch} />
                            {cell.latest ? <Status status={cell.latest.status} /> : <span className="rnd-dim">Never run</span>}
                        </div>
                        <div className="rnd-compare-foot">
                            <span className="rnd-compare-when">{cell.latest ? `${cell.latest.user} · ${when ? formatAgo(when, now) : ""}` : ""}</span>
                            {drift && (
                                <span className="rnd-drift">
                                    <IconWarning size={11} />
                                    live is {cell.deployed!.branch}
                                </span>
                            )}
                        </div>
                        {cell.enabled !== false && (
                            <button
                                className="rnd-btn rnd-row-deploy rnd-compare-deploy"
                                onClick={(e) => {
                                    e.stopPropagation();
                                    onDeploy(cell);
                                }}>
                                <IconRun size={10} />
                                {cell.latest?.branch || cell.deployed?.branch ? "Deploy" : "Run"}
                            </button>
                        )}
                    </div>
                );
            })}
        </>
    );
}
