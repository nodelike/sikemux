import { createContext, useContext, type ReactNode } from "react";
import { IconChevron, IconGit, IconPanelLeft, IconRefresh, IconSearch, Tooltip } from "../../../plugin-api/ui";
import * as cmd from "../state";
import type { JobRef, RundeckLevel } from "../state";
import { groupSegments, isProdTarget } from "../shape";
import { statusKind } from "./branchStyle";

/** The Rundeck server the pane is signed in to, as a host name. */
export const RundeckHost = createContext<string | null>(null);

export function hostFromUrl(url: string): string {
    try {
        return new URL(url).host;
    } catch {
        return url;
    }
}

interface Crumb {
    key: string;
    label: string;
    onClick?: () => void;
}

/** Shows a job picked out in its folder's list, the way a row click does. */
export function showJob(paneId: string, job: JobRef): void {
    const { stack } = cmd.rundeckView(paneId);
    const index = stack.findIndex((level) => level.kind === "service" && level.jobId === job.jobId);
    if (index >= 0) return cmd.rundeckPopTo(paneId, index);
    cmd.updateRundeckSettings({ activeProject: job.project, activeGroup: job.group });
    cmd.selectRundeckJob(paneId, job.jobId);
    cmd.rundeckHome(paneId);
}

function locationCrumbs(paneId: string, project: string, group: string | null): Crumb[] {
    if (!project) return [];
    const segments = groupSegments(group);
    return [
        { key: `project-${project}`, label: project, onClick: () => cmd.selectRundeckGroup(paneId, project, null) },
        ...segments.map((segment, index) => {
            const path = segments.slice(0, index + 1).join("/");
            return { key: `group-${path}`, label: segment, onClick: () => cmd.selectRundeckGroup(paneId, project, path) };
        }),
    ];
}

export function levelCrumbs(paneId: string, level: RundeckLevel, project: string, group: string | null, runWord = "deploy"): Crumb[] {
    if (level.kind === "matrix" || level.kind === "service") return locationCrumbs(paneId, project, group);
    const job: Crumb = { key: `job-${level.jobId}`, label: level.name, onClick: () => showJob(paneId, level) };
    const base = [...locationCrumbs(paneId, level.project, level.group), job];
    if (level.kind === "deploy") return [...base, { key: "run", label: runWord }];
    return [...base, { key: `execution-${level.executionId}`, label: `#${level.executionId}` }];
}

export function Header({
    paneId,
    crumbs,
    title,
    count,
    aside,
    tools,
    onRefresh,
    refreshing,
}: {
    paneId: string;
    crumbs: Crumb[];
    title: ReactNode;
    count?: number;
    aside?: ReactNode;
    tools?: ReactNode;
    onRefresh?: () => void;
    refreshing?: boolean;
}) {
    const host = useContext(RundeckHost);
    const treeHidden = cmd.rundeckSettings.useSelect((s) => s.treeHidden);
    const { stack } = cmd.useRundeckView(paneId);
    return (
        <header className="rnd-head">
            <nav className="rnd-crumbs" aria-label="Rundeck location">
                {treeHidden && (
                    <Tooltip label="Show projects">
                        <button
                            className="rnd-crumbs-tree"
                            onClick={() => cmd.updateRundeckSettings({ treeHidden: false })}
                            aria-label="Show projects">
                            <IconPanelLeft size={12} />
                        </button>
                    </Tooltip>
                )}
                {stack.length > 1 && (
                    <button className="rnd-crumbs-back" onClick={() => cmd.rundeckPop(paneId)} aria-label="Back" title="Back">
                        <IconChevron size={10} />
                    </button>
                )}
                {host && <span className="rnd-crumbs-host">{host}</span>}
                {crumbs.map((crumb, index) => {
                    const last = index === crumbs.length - 1;
                    return (
                        <span key={crumb.key} className="rnd-crumbs-part">
                            {(index > 0 || host) && <IconChevron size={9} />}
                            {crumb.onClick && !last ? (
                                <button className="rnd-crumbs-link" onClick={crumb.onClick}>
                                    {crumb.label}
                                </button>
                            ) : (
                                <span aria-current={last ? "page" : undefined}>{crumb.label}</span>
                            )}
                        </span>
                    );
                })}
            </nav>
            <div className="rnd-head-row">
                <h2 className="rnd-title">
                    {title}
                    {count !== undefined && <span className="rnd-count">{count}</span>}
                </h2>
                {aside}
                <div className="rnd-head-tools">
                    {tools}
                    {onRefresh && (
                        <button
                            className={`rnd-btn rnd-icon-btn${refreshing ? " busy" : ""}`}
                            onClick={onRefresh}
                            disabled={refreshing}
                            aria-label="Refresh"
                            title="Refresh">
                            <IconRefresh size={14} />
                        </button>
                    )}
                </div>
            </div>
        </header>
    );
}

export interface SegOption<V extends string> {
    value: V;
    label: ReactNode;
    count?: number;
}

export function Seg<V extends string>({
    options,
    value,
    onChange,
    label,
}: {
    options: SegOption<V>[];
    value: V;
    onChange: (value: V) => void;
    label: string;
}) {
    return (
        <div className="rnd-seg" role="group" aria-label={label}>
            {options.map((o) => (
                <button
                    key={o.value}
                    type="button"
                    className={o.value === value ? "on" : ""}
                    aria-pressed={o.value === value}
                    onClick={() => onChange(o.value)}>
                    {o.label}
                    {o.count !== undefined && <span className="rnd-seg-count">{o.count}</span>}
                </button>
            ))}
        </div>
    );
}

export function Filter({ value, onChange, placeholder }: { value: string; onChange: (value: string) => void; placeholder: string }) {
    return (
        <label className="rnd-filter">
            <IconSearch size={14} />
            <input
                value={value}
                placeholder={placeholder}
                spellCheck={false}
                onChange={(e) => onChange(e.target.value)}
                onKeyDown={(e) => {
                    if (e.key === "Escape" && value) {
                        e.stopPropagation();
                        onChange("");
                    }
                }}
            />
            <kbd>/</kbd>
        </label>
    );
}

const STATUS_LABEL: Record<ReturnType<typeof statusKind>, string> = {
    succeeded: "Succeeded",
    failed: "Failed",
    running: "Running",
    aborted: "Aborted",
    unknown: "Unknown",
};

/** A run's status as a dot and a word; `label` overrides the word, and an empty label leaves the dot alone. */
export function Status({ status, label }: { status: string | null | undefined; label?: string }) {
    const kind = statusKind(status);
    const custom = kind === "unknown" && status ? status : null;
    return (
        <span className={`rnd-status ${kind}`}>
            <i />
            {label ?? custom ?? STATUS_LABEL[kind]}
        </span>
    );
}

export function BranchChip({ branch }: { branch: string | null | undefined }) {
    if (!branch) return <span className="rnd-dim">—</span>;
    const kind = branch.startsWith("feat") || branch.startsWith("feature") ? "feat" : /^(release|hotfix)\//.test(branch) ? "rel" : "";
    return (
        <span className={`rnd-branch${kind ? ` ${kind}` : ""}`} title={branch}>
            <IconGit size={12} />
            <span>{branch}</span>
        </span>
    );
}

/** A job's folder as a path. Amber when the folder matches the production names in settings. */
export function FolderChip({ project, group }: { project: string; group: string | null }) {
    const prod = cmd.rundeckSettings.useSelect((s) => isProdTarget(project, group, s.prodEnvs));
    const path = groupSegments(group).join("/") || project;
    return <span className={`rnd-folder${prod ? " prod" : ""}`}>{path}</span>;
}

export function Tag({ children, tone }: { children: ReactNode; tone?: "warn" | "muted" }) {
    return <span className={`rnd-tag${tone ? ` ${tone}` : ""}`}>{children}</span>;
}
