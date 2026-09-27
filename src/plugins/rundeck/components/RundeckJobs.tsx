import { useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { git, openUrl, swallow, useActiveProjectCwd } from "../../../plugin-api/host";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { EmptyState, IconChevron, IconExternal, IconFetch, IconGit, IconRun, IconWarning, SkeletonRows, Tooltip } from "../../../plugin-api/ui";
import { errorMessage, type MatrixCell, type RundeckExecution } from "../api";
import { rndExecutionsR, rndJobDetailR, rndJobsR, rndMatrixR } from "../resources";
import * as cmd from "../state";
import type { JobRef, RundeckLevel } from "../state";
import { branchOf, branchOptionName, childSegment, duration, groupSegments, inGroup, isLiveStatus, isProdTarget, relativeTime } from "../shape";
import { statusKind } from "./branchStyle";
import { newestExecutions } from "./executionProgress";
import { useNow } from "./hooks";
import { reusableOptions } from "./options";
import { BranchChip, Filter, FolderChip, Header, Seg, Status, Tag, levelCrumbs } from "./parts";

interface Props {
    paneId: string;
    active: boolean;
    level: Extract<RundeckLevel, { kind: "matrix" } | { kind: "service" }>;
}

type StatusFilter = "all" | "running" | "failed";

interface Section {
    /** A folder under the current one, or null for jobs that sit in it directly. */
    segment: string | null;
    cells: MatrixCell[];
}

export function RundeckJobs({ paneId, active, level }: Props) {
    const settingsProject = cmd.rundeckSettings.useSelect((s) => s.activeProject);
    const settingsGroup = cmd.rundeckSettings.useSelect((s) => s.activeGroup);
    const branchOptions = cmd.rundeckSettings.useSelect((s) => s.branchOptions);
    const project = level.kind === "service" ? level.project : settingsProject;
    const group = level.kind === "service" ? level.group : settingsGroup;
    const picked = cmd.useRundeck((s) => s.selectedJob[paneId]);
    const layoutChoice = cmd.useRundeck((s) => s.layout[paneId] ?? "list");
    const activeCwd = useActiveProjectCwd();
    const [statusFilter, setStatusFilter] = useState<StatusFilter>("all");
    const [query, setQuery] = useState("");

    const res = useResourceEnabled(active && !!project, rndMatrixR, project, branchOptions);
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
    const selectedId = level.kind === "service" ? level.jobId : picked;
    const selected = cells.find((c) => c.job_id === selectedId) ?? flat[0] ?? null;

    const jobRef = (cell: MatrixCell): JobRef => ({
        project,
        jobId: cell.job_id,
        name: cell.name,
        group: cell.group,
        repoPath: cmd.linkedRepoPath({ jobId: cell.job_id, name: cell.name }, activeCwd),
    });
    const select = (cell: MatrixCell) => {
        if (level.kind === "service") cmd.rundeckReplace(paneId, { kind: "service", ...jobRef(cell) });
        else cmd.selectRundeckJob(paneId, cell.job_id);
    };
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
        <>
            <div className="rnd-main">
                <Header
                    paneId={paneId}
                    crumbs={levelCrumbs(paneId, level, project, group)}
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
                        Rundeck was slow to answer{timedOut ? `: ${timedOut} job${timedOut === 1 ? "" : "s"} timed out` : ""}. Those rows show no
                        recent runs; refresh to try again.
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
                    <JobList
                        paneId={paneId}
                        project={project}
                        group={group}
                        sections={shownList}
                        selectedId={selected?.job_id ?? null}
                        now={now}
                        onSelect={select}
                        onDeploy={openForm}
                    />
                )}
                {layout === "compare" && flat.length > 0 && (
                    <Compare
                        project={project}
                        group={group}
                        sections={shown.filter((s) => s.segment !== null)}
                        loose={shown.find((s) => s.segment === null)?.cells.length ?? 0}
                        selectedId={selected?.job_id ?? null}
                        now={now}
                        onSelect={select}
                        onDeploy={openForm}
                    />
                )}
            </div>
            {selected ? (
                <JobInspector key={selected.job_id} paneId={paneId} active={active} cell={selected} job={jobRef(selected)} now={now} />
            ) : (
                <aside className="rnd-insp" />
            )}
        </>
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
    selectedId,
    now,
    onSelect,
    onDeploy,
}: {
    paneId: string;
    project: string;
    group: string | null;
    sections: Section[];
    selectedId: string | null;
    now: number;
    onSelect: (cell: MatrixCell) => void;
    onDeploy: (cell: MatrixCell) => void;
}) {
    const body = useRef<HTMLTableSectionElement>(null);
    const prodEnvs = cmd.rundeckSettings.useSelect((s) => s.prodEnvs);
    const flat = sections.flatMap((s) => s.cells);
    const onKey = (event: KeyboardEvent, cell: MatrixCell) => {
        const at = flat.indexOf(cell);
        if (event.key === "Enter") {
            event.preventDefault();
            onDeploy(cell);
        } else if (event.key === "ArrowDown" || event.key === "ArrowUp") {
            event.preventDefault();
            const next = flat[Math.max(0, Math.min(flat.length - 1, at + (event.key === "ArrowDown" ? 1 : -1)))];
            if (!next) return;
            onSelect(next);
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
                        <th>Last run</th>
                        <th>By</th>
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
                                    const sel = cell.job_id === selectedId;
                                    const when = lastRunTime(cell);
                                    const disabled = cell.enabled === false;
                                    return (
                                        <tr
                                            key={cell.job_id}
                                            data-job={cell.job_id}
                                            tabIndex={0}
                                            aria-selected={sel}
                                            className={`${sel ? "sel" : ""}${disabled ? " dim" : ""}`}
                                            onClick={() => onSelect(cell)}
                                            onKeyDown={(e) => onKey(e, cell)}>
                                            <td>
                                                <span className="rnd-name-row">
                                                    <span className="rnd-name">{cell.name}</span>
                                                    {cell.scheduled && <Tag>scheduled</Tag>}
                                                    {disabled && <Tag tone="muted">disabled</Tag>}
                                                    {cell.error && <Tag tone="warn">{cell.error === "timed out" ? "timed out" : "error"}</Tag>}
                                                </span>
                                            </td>
                                            <td>
                                                <BranchChip branch={cell.latest?.branch ?? cell.deployed?.branch} />
                                            </td>
                                            <td>
                                                {cell.latest ? <Status status={cell.latest.status} /> : <span className="rnd-dim">Never run</span>}
                                            </td>
                                            <td className="rnd-cell-sub">{cell.latest?.user ?? "—"}</td>
                                            <td className="r rnd-cell-dim">{when ? relativeTime(when, now) : "—"}</td>
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
            <col style={{ width: "28%" }} />
            <col style={{ width: "22%" }} />
            <col style={{ width: "16%" }} />
            <col style={{ width: "13%" }} />
            <col style={{ width: "11%" }} />
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
                    <td colSpan={7}>
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
    selectedId,
    now,
    onSelect,
    onDeploy,
}: {
    project: string;
    group: string | null;
    sections: Section[];
    loose: number;
    selectedId: string | null;
    now: number;
    onSelect: (cell: MatrixCell) => void;
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
                    <CompareRow
                        key={name}
                        name={name}
                        sections={sections}
                        selectedId={selectedId}
                        now={now}
                        onSelect={onSelect}
                        onDeploy={onDeploy}
                    />
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
    selectedId,
    now,
    onSelect,
    onDeploy,
}: {
    name: string;
    sections: Section[];
    selectedId: string | null;
    now: number;
    onSelect: (cell: MatrixCell) => void;
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
                        className={`rnd-compare-cell${cell.job_id === selectedId ? " sel" : ""}`}
                        role="button"
                        tabIndex={0}
                        aria-pressed={cell.job_id === selectedId}
                        onClick={() => onSelect(cell)}
                        onKeyDown={(e) => {
                            if (e.key === "Enter" || e.key === " ") {
                                e.preventDefault();
                                onSelect(cell);
                            }
                        }}>
                        <div className="rnd-compare-top">
                            <BranchChip branch={cell.latest?.branch ?? cell.deployed?.branch} />
                            {cell.latest ? <Status status={cell.latest.status} /> : <span className="rnd-dim">Never run</span>}
                        </div>
                        <div className="rnd-compare-foot">
                            <span className="rnd-compare-when">
                                {cell.latest ? `${cell.latest.user} · ${when ? relativeTime(when, now) : ""}` : ""}
                            </span>
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

const RECENT = 8;
const SPARK = 10;

function JobInspector({ paneId, active, cell, job, now }: { paneId: string; active: boolean; cell: MatrixCell; job: JobRef; now: number }) {
    const branchOptions = cmd.rundeckSettings.useSelect((s) => s.branchOptions);
    const isProd = cmd.rundeckSettings.useSelect((s) => isProdTarget(job.project, job.group, s.prodEnvs));
    const detail = useResourceEnabled(active, rndJobDetailR, job.jobId);
    const jobs = useResourceEnabled(active, rndJobsR, job.project);
    const execs = useResourceEnabled(active, rndExecutionsR, job.jobId, job.project, 25);
    const [actionError, setActionError] = useState<string | null>(null);
    const refresh = useRef(execs.refresh);
    refresh.current = execs.refresh;

    const executions = useMemo(() => newestExecutions(execs.data ?? []), [execs.data]);
    const anyLive = executions.some((ex) => isLiveStatus(ex.status));
    const summary = jobs.data?.find((j) => j.id === job.jobId) ?? null;
    const branchKey = detail.data
        ? branchOptionName(
              detail.data.options.map((o) => o.name),
              branchOptions,
          )
        : null;
    const runsBranch = detail.data
        ? branchKey !== null
        : !!cell.latest?.branch || !!cell.deployed?.branch || executions.some((ex) => branchOf(ex.job?.options, branchOptions) !== null);
    const runWord = runsBranch ? "Deploy" : "Run";
    const executionEnabled = detail.data?.execution_enabled !== false && cell.enabled !== false;
    const secureNames = useMemo(() => new Set(detail.data?.options.filter((o) => o.secure).map((o) => o.name) ?? []), [detail.data]);
    const lastGood = executions.find((ex) => ex.status === "succeeded" && (!runsBranch || branchOf(ex.job?.options, branchOptions) !== null)) ?? null;
    const lastGoodBranch = lastGood ? branchOf(lastGood.job?.options, branchOptions) : null;

    useEffect(() => {
        if (!active) return;
        const tick = () => {
            if (!document.hidden) void refresh.current().catch(() => {});
        };
        const timer = window.setInterval(tick, anyLive ? 3_000 : 20_000);
        document.addEventListener("visibilitychange", tick);
        return () => {
            window.clearInterval(timer);
            document.removeEventListener("visibilitychange", tick);
        };
    }, [active, anyLive]);

    const openForm = (branch: string | undefined, options?: Record<string, string>) => {
        setActionError(null);
        cmd.rundeckPush(paneId, { kind: "deploy", ...job, branch, options });
    };
    const deployCheckout = async () => {
        if (!job.repoPath) return;
        setActionError(null);
        try {
            const status = await git.status(job.repoPath);
            openForm(status.branch === "HEAD" ? "" : status.branch);
        } catch (e) {
            setActionError(errorMessage(e));
        }
    };
    const openRun = (executionId: number) => cmd.rundeckPush(paneId, { kind: "execution", ...job, executionId });

    const spark = executions.slice(0, SPARK).reverse();
    const longest = Math.max(1, ...spark.map((ex) => runSeconds(ex, now)));
    const succeeded = executions.slice(0, SPARK).filter((ex) => ex.status === "succeeded").length;
    const deployed = cell.deployed;

    return (
        <aside className="rnd-insp">
            <div className="rnd-insp-head">
                <div className="rnd-insp-title">
                    <span className="rnd-insp-name">{job.name}</span>
                    <FolderChip project={job.project} group={job.group} />
                </div>
                {detail.data?.description && <p className="rnd-insp-desc">{detail.data.description}</p>}
                {(cell.scheduled || detail.data?.node_filter || !executionEnabled) && (
                    <div className="rnd-insp-tags">
                        {cell.scheduled && <Tag tone={detail.data?.schedule_enabled === false ? "muted" : undefined}>scheduled</Tag>}
                        {!executionEnabled && <Tag tone="warn">executions disabled</Tag>}
                        {detail.data?.node_filter && <Tag>nodes: {detail.data.node_filter}</Tag>}
                    </div>
                )}
                <div className="rnd-insp-actions">
                    <button className={`rnd-btn ${isProd ? "prod" : "primary"}`} onClick={() => openForm(undefined)} disabled={!executionEnabled}>
                        <IconRun size={11} />
                        {runWord}
                    </button>
                    <Tooltip label={lastGood ? `Prefill from #${lastGood.id}` : `No successful ${runWord.toLowerCase()} to repeat yet`}>
                        <span className="rnd-btn-wrap">
                            <button
                                className="rnd-btn"
                                onClick={() => lastGood && openForm(lastGoodBranch ?? undefined, reusableOptions(lastGood.job?.options, secureNames))}
                                disabled={!lastGood || !executionEnabled}>
                                <IconFetch size={13} />
                                {runsBranch ? `Redeploy${lastGoodBranch ? ` ${lastGoodBranch}` : ""}` : "Run again"}
                            </button>
                        </span>
                    </Tooltip>
                    {runsBranch && job.repoPath && (
                        <Tooltip label={`Deploy the branch checked out in ${job.repoPath}`}>
                            <button
                                className="rnd-btn rnd-icon-btn"
                                onClick={() => void deployCheckout()}
                                disabled={!executionEnabled}
                                aria-label="Deploy the checked-out branch">
                                <IconGit size={13} />
                            </button>
                        </Tooltip>
                    )}
                    {summary?.permalink && (
                        <Tooltip label="Open in Rundeck">
                            <button
                                className="rnd-btn rnd-icon-btn"
                                onClick={() => void openUrl(summary.permalink!).catch(swallow("open Rundeck URL"))}
                                aria-label="Open in Rundeck">
                                <IconExternal size={13} />
                            </button>
                        </Tooltip>
                    )}
                </div>
                {actionError && <div className="rnd-banner danger">{actionError}</div>}
                {detail.error && <div className="rnd-insp-note">Couldn't read the job's options: {detail.error}</div>}
            </div>

            {runsBranch && deployed?.branch && (
                <div className="rnd-insp-sec">
                    <button className="rnd-live-row" onClick={() => openRun(deployed.execution_id)} title={`Open run #${deployed.execution_id}`}>
                        <span className="rnd-live-k">Live</span>
                        <BranchChip branch={deployed.branch} />
                        <span className="rnd-live-m">
                            #{deployed.execution_id}
                            {deployed.ended_at ? ` · ${relativeTime(deployed.ended_at, now)}` : ""}
                        </span>
                        <IconChevron size={11} />
                    </button>
                </div>
            )}

            {spark.length > 0 && (
                <div className="rnd-insp-sec">
                    <h3>
                        Last {spark.length} runs
                        <span>
                            {succeeded}/{spark.length} succeeded
                        </span>
                    </h3>
                    <div className="rnd-spark" aria-label={`${succeeded} of ${spark.length} runs succeeded`}>
                        {spark.map((ex) => (
                            <i
                                key={ex.id}
                                className={statusKind(ex.status)}
                                style={{ height: `${30 + (runSeconds(ex, now) / longest) * 70}%` }}
                                title={`#${ex.id} · ${ex.status ?? "unknown"} · ${duration(started(ex), ended(ex), now)}`}
                            />
                        ))}
                    </div>
                </div>
            )}

            <div className="rnd-insp-sec">
                <h3>
                    Recent runs
                    {execs.data && <span>{executions.length}</span>}
                </h3>
                {execs.error && <div className="rnd-insp-note err">{execs.error}</div>}
                {execs.status === "loading" && !execs.data && <div className="rnd-insp-note">Loading runs…</div>}
                {execs.data && executions.length === 0 && <div className="rnd-insp-note">No runs yet.</div>}
                <div className="rnd-runs">
                    {executions.slice(0, RECENT).map((ex) => (
                        <button key={ex.id} className="rnd-run" onClick={() => openRun(ex.id)} title={`Open run #${ex.id}`}>
                            <Status status={ex.status} label="" />
                            <span className="rnd-run-id">#{ex.id}</span>
                            <span className="rnd-run-branch">{branchOf(ex.job?.options, branchOptions) ?? ex.user ?? ""}</span>
                            <span className="rnd-run-when">{started(ex) ? relativeTime(started(ex)!, now) : ""}</span>
                            <span className="rnd-run-dur">{duration(started(ex), ended(ex), now)}</span>
                            <IconChevron size={11} />
                        </button>
                    ))}
                </div>
            </div>
        </aside>
    );
}

const started = (ex: RundeckExecution) => ex["date-started"]?.date ?? null;
const ended = (ex: RundeckExecution) => ex["date-ended"]?.date ?? null;

function runSeconds(ex: RundeckExecution, now: number): number {
    const a = Date.parse(started(ex) ?? "");
    const b = ended(ex) ? Date.parse(ended(ex)!) : now;
    return Number.isNaN(a) || Number.isNaN(b) ? 0 : Math.max(0, (b - a) / 1000);
}
