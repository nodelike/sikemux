import { useMemo } from "react";
import * as cmd from "../state";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { rndJobIndexR, rndMatrixR, rndProjectsR } from "../resources";
import { IconChevron, IconFolder, IconPanelLeft, IconSearch, Tooltip } from "../../../plugin-api/ui";
import type { RundeckJob } from "../api";
import { groupSegments, isLiveStatus } from "../shape";
import { ancestorPaths, buildGroupTree, type GroupNode } from "./groupTree";

export function RundeckProjectTree({ paneId, active }: { paneId: string; active: boolean }) {
    const projects = useResourceEnabled(active, rndProjectsR);
    const index = useResourceEnabled(active, rndJobIndexR);
    const settingsProject = cmd.rundeckSettings.useSelect((s) => s.activeProject);
    const settingsGroup = cmd.rundeckSettings.useSelect((s) => s.activeGroup);
    const { stack } = cmd.useRundeckView(paneId);
    const top = stack[stack.length - 1];
    const onJob = top && top.kind !== "matrix";
    const activeProject = onJob ? top.project : settingsProject;
    const activeGroup = onJob ? top.group : settingsGroup;
    const branchOptions = cmd.rundeckSettings.useSelect((s) => s.branchOptions);
    const openState = cmd.useRundeck((s) => s.treeOpen[paneId]);
    const matrix = useResourceEnabled(active && !!activeProject, rndMatrixR, activeProject, branchOptions);

    const jobsByProject = useMemo(() => {
        const map = new Map<string, { jobs: RundeckJob[]; error: string | null }>();
        for (const entry of index.data ?? []) map.set(entry.project, { jobs: entry.jobs, error: entry.error });
        return map;
    }, [index.data]);

    const names = useMemo(() => {
        const list = projects.data?.map((p) => p.name) ?? index.data?.map((entry) => entry.project) ?? [];
        return [...list].sort((a, b) => a.localeCompare(b));
    }, [projects.data, index.data]);

    /** Folders in the active project with a run going somewhere inside them; "" is the project itself. */
    const livePaths = useMemo(() => {
        const paths = new Set<string>();
        for (const cell of matrix.data?.cells ?? []) {
            if (!isLiveStatus(cell.latest?.status)) continue;
            paths.add("");
            for (const path of ancestorPaths(cell.group)) paths.add(path);
        }
        return paths;
    }, [matrix.data]);

    const activePaths = useMemo(() => new Set(ancestorPaths(activeGroup)), [activeGroup]);

    const isOpen = (project: string, path: string | null): boolean => {
        const explicit = openState?.[cmd.treeKey(project, path)];
        if (explicit !== undefined) return explicit;
        return project === activeProject && (path === null || activePaths.has(path));
    };

    const rows: RowProps[] = [];
    for (const project of names) {
        const entry = jobsByProject.get(project);
        const tree = buildGroupTree(entry?.jobs ?? []);
        const open = isOpen(project, null);
        rows.push({
            paneId,
            project,
            path: null,
            name: project,
            depth: 0,
            count: entry ? tree.total : null,
            expandable: tree.children.length > 0,
            open,
            selected: project === activeProject && activeGroup === null,
            live: project === activeProject && livePaths.has(""),
            error: entry?.error ?? null,
        });
        if (open)
            addGroups(
                rows,
                paneId,
                project,
                tree.children,
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
                    <TreeRow key={`${row.project}\u0000${row.path ?? ""}`} {...row} />
                ))}
                {names.length === 0 && projects.status === "loading" && <div className="rnd-tree-hint">Loading projects…</div>}
                {names.length === 0 && projects.status === "ok" && <div className="rnd-tree-hint">No projects</div>}
                {projects.error && !projects.data && <div className="rnd-tree-hint err">{projects.error}</div>}
                {index.error && !index.data && <div className="rnd-tree-hint err">{index.error}</div>}
            </div>
        </aside>
    );
}

function addGroups(
    rows: RowProps[],
    paneId: string,
    project: string,
    nodes: GroupNode[],
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
}

function TreeRow({ paneId, project, path, name, depth, count, expandable, open, selected, live, error }: RowProps) {
    const select = () => {
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
            title={error ?? (path ? `${project} · ${path}` : project)}
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
                <span className="rnd-tree-icon">
                    <IconFolder size={14} />
                </span>
                <span className="rnd-tree-name">{name}</span>
                {live && <span className="rnd-live-dot" title="A job in here is running" />}
                {error ? <span className="rnd-tree-count err">!</span> : count !== null && <span className="rnd-tree-count">{count}</span>}
            </span>
        </div>
    );
}
