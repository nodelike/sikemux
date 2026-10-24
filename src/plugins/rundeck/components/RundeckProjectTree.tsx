import { useMemo } from "react";
import * as cmd from "../state";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { rndJobsV, rndTreeV } from "../resources";
import { IconActivity, IconChevron, IconFolder, IconPanelLeft, IconSearch, Tooltip } from "../../../plugin-api/ui";
import type { RundeckGroupNode } from "../types.generated";
import { groupSegments } from "../shape";

/** Every folder that must be open for `path` to be visible, including `path` itself. */
function ancestorPaths(path: string | null): string[] {
    const segments = groupSegments(path);
    return segments.map((_, index) => segments.slice(0, index + 1).join("/"));
}

export function RundeckProjectTree({ paneId, active }: { paneId: string; active: boolean }) {
    const tree = useResourceEnabled(active, rndTreeV, `${paneId}/tree`);
    if (tree.data) cmd.markRundeckLooked();
    const settingsProject = cmd.rundeckSettings.useSelect((s) => s.activeProject);
    const settingsGroup = cmd.rundeckSettings.useSelect((s) => s.activeGroup);
    const { stack } = cmd.useRundeckView(paneId);
    const top = stack[stack.length - 1];
    const onActivity = top?.kind === "activity";
    const activeProject = top && top.kind !== "matrix" ? top.project : settingsProject;
    const activeGroup = top && top.kind !== "matrix" && top.kind !== "activity" ? top.group : onActivity ? null : settingsGroup;
    const openState = cmd.useRundeck((s) => s.treeOpen[paneId]);
    const jobs = useResourceEnabled(active && !!activeProject, rndJobsV, `${paneId}/tree-live`, activeProject, activeGroup ?? "");

    /** Folders in the active project with a run going somewhere inside them; "" is the project itself. */
    const livePaths = useMemo(() => new Set(jobs.data?.livePaths ?? []), [jobs.data]);

    const activePaths = useMemo(() => new Set(ancestorPaths(activeGroup)), [activeGroup]);

    const isOpen = (project: string, path: string | null): boolean => {
        const explicit = openState?.[cmd.treeKey(project, path)];
        if (explicit !== undefined) return explicit;
        return project === activeProject && (path === null || activePaths.has(path));
    };

    const rows: RowProps[] = [];
    for (const entry of tree.data?.projects ?? []) {
        const project = entry.name;
        const open = isOpen(project, null);
        rows.push({
            paneId,
            project,
            path: null,
            name: project,
            depth: 0,
            count: entry.error ? null : entry.total,
            expandable: entry.total > 0,
            open,
            selected: !onActivity && project === activeProject && activeGroup === null,
            live: project === activeProject && livePaths.has(""),
            error: entry.error,
        });
        if (open && entry.total > 0)
            rows.push({
                paneId,
                project,
                path: null,
                name: "Activity",
                depth: 1,
                count: null,
                expandable: false,
                open: false,
                selected: onActivity && project === activeProject,
                live: project === activeProject && livePaths.has(""),
                error: null,
                activity: true,
            });
        if (open)
            addGroups(
                rows,
                paneId,
                project,
                entry.groups,
                1,
                isOpen,
                project === activeProject ? activeGroup : undefined,
                project === activeProject ? livePaths : null,
            );
    }

    return (
        <aside className="rnd-tree" aria-label="Rundeck projects">
            <div className="rnd-tree-head">
                <span>Projects</span>
                <Tooltip label="Search every job">
                    <button className="rnd-tree-tool" onClick={cmd.openRundeckJobPalette} aria-label="Search every job">
                        <IconSearch size={12} />
                    </button>
                </Tooltip>
                <Tooltip label="Hide projects">
                    <button className="rnd-tree-tool" onClick={() => cmd.updateRundeckSettings({ treeHidden: true })} aria-label="Hide projects">
                        <IconPanelLeft size={12} />
                    </button>
                </Tooltip>
            </div>
            <div className="rnd-tree-rows" role="tree">
                {rows.map((row) => (
                    <TreeRow key={`${row.project}\u0000${row.activity ? "\u0001activity" : (row.path ?? "")}`} {...row} />
                ))}
                {!tree.data && tree.status === "loading" && <div className="rnd-tree-hint">Loading projects…</div>}
                {tree.data?.projects.length === 0 && <div className="rnd-tree-hint">No projects</div>}
                {tree.error && !tree.data && <div className="rnd-tree-hint err">{tree.error}</div>}
            </div>
        </aside>
    );
}

function addGroups(
    rows: RowProps[],
    paneId: string,
    project: string,
    nodes: RundeckGroupNode[],
    depth: number,
    isOpen: (project: string, path: string | null) => boolean,
    activeGroup: string | null | undefined,
    livePaths: Set<string> | null,
): void {
    for (const node of nodes) {
        const open = isOpen(project, node.path);
        rows.push({
            paneId,
            project,
            path: node.path,
            name: node.name,
            depth,
            count: node.count,
            expandable: node.children.length > 0,
            open,
            selected: activeGroup !== undefined && groupSegments(activeGroup).join("/") === node.path,
            live: !!livePaths?.has(node.path),
            error: null,
        });
        if (open) addGroups(rows, paneId, project, node.children, depth + 1, isOpen, activeGroup, livePaths);
    }
}

interface RowProps {
    paneId: string;
    project: string;
    path: string | null;
    name: string;
    depth: number;
    count: number | null;
    expandable: boolean;
    open: boolean;
    selected: boolean;
    live: boolean;
    error: string | null;
    /** The project's every-run list rather than a folder. */
    activity?: boolean;
}

function TreeRow({ paneId, project, path, name, depth, count, expandable, open, selected, live, error, activity }: RowProps) {
    const select = () => {
        if (activity) return cmd.openRundeckActivity(paneId, project);
        cmd.setTreeOpen(paneId, cmd.treeKey(project, path), true);
        cmd.selectRundeckGroup(paneId, project, path);
    };
    return (
        <div
            className="rnd-tree-row"
            role="treeitem"
            aria-level={depth + 1}
            aria-selected={selected}
            aria-expanded={expandable ? open : undefined}
            tabIndex={0}
            title={activity ? `Every run in ${project}` : (error ?? (path ? `${project} · ${path}` : project))}
            onClick={select}
            onKeyDown={(event) => {
                if (event.key === "Enter" || event.key === " ") {
                    event.preventDefault();
                    select();
                } else if (expandable && (event.key === "ArrowRight" || event.key === "ArrowLeft")) {
                    event.preventDefault();
                    cmd.setTreeOpen(paneId, cmd.treeKey(project, path), event.key === "ArrowRight");
                }
            }}>
            {Array.from({ length: depth }, (_, i) => (
                <span key={i} className="rnd-tree-guide" />
            ))}
            <span className={`rnd-tree-item${selected ? " on" : ""}`}>
                {expandable ? (
                    <button
                        type="button"
                        className={`rnd-tree-twist${open ? " open" : ""}`}
                        tabIndex={-1}
                        aria-label={open ? "Collapse" : "Expand"}
                        onClick={(event) => {
                            event.stopPropagation();
                            cmd.setTreeOpen(paneId, cmd.treeKey(project, path), !open);
                        }}>
                        <IconChevron size={10} />
                    </button>
                ) : (
                    <span className="rnd-tree-twist" />
                )}
                <span className="rnd-tree-icon">{activity ? <IconActivity size={14} /> : <IconFolder size={14} />}</span>
                <span className="rnd-tree-name">{name}</span>
                {live && <span className="rnd-live-dot" title={activity ? "Something is running" : "A job in here is running"} />}
                {error ? <span className="rnd-tree-count err">!</span> : count !== null && <span className="rnd-tree-count">{count}</span>}
            </span>
        </div>
    );
}
