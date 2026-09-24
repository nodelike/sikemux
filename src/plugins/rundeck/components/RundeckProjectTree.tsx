import { useMemo } from "react";
import * as cmd from "../state";
import { useResourceEnabled } from "../../../plugin-api/resources";
import { rndJobIndexR, rndProjectsR } from "../resources";
import { IconChevron, IconFolder } from "../../../plugin-api/ui";
import type { RundeckJob } from "../api";
import { ancestorPaths, buildGroupTree, type GroupNode } from "./groupTree";

export function RundeckProjectTree({ paneId, active }: { paneId: string; active: boolean }) {
    const projects = useResourceEnabled(active, rndProjectsR);
    const index = useResourceEnabled(active, rndJobIndexR);
    const width = cmd.useRundeck((s) => s.treeWidth);
    const activeProject = cmd.rundeckSettings.useSelect((s) => s.activeProject);
    const activeGroup = cmd.rundeckSettings.useSelect((s) => s.activeGroup);
    const openState = cmd.useRundeck((s) => s.treeOpen[paneId]);

    const jobsByProject = useMemo(() => {
        const map = new Map<string, { jobs: RundeckJob[]; error: string | null }>();
        for (const entry of index.data ?? []) map.set(entry.project, { jobs: entry.jobs, error: entry.error });
        return map;
    }, [index.data]);

    const names = useMemo(() => {
        const list = projects.data?.map((p) => p.name) ?? index.data?.map((entry) => entry.project) ?? [];
        return [...list].sort((a, b) => a.localeCompare(b));
    }, [projects.data, index.data]);

    const activePaths = useMemo(() => new Set(ancestorPaths(activeGroup)), [activeGroup]);

    const isOpen = (project: string, path: string | null): boolean => {
        const explicit = openState?.[cmd.treeKey(project, path)];
        if (explicit !== undefined) return explicit;
        return project === activeProject && (path === null || activePaths.has(path));
    };

    return (
        <aside className="rnd-tree" style={{ width }} aria-label="Rundeck projects">
            <div className="rnd-tree-section" role="tree">
                {names.map((project) => (
                    <ProjectBranch
                        key={project}
                        paneId={paneId}
                        project={project}
                        entry={jobsByProject.get(project)}
                        indexLoading={index.status === "loading" && !index.data}
                        activeProject={activeProject}
                        activeGroup={activeGroup}
                        isOpen={isOpen}
                    />
                ))}
                {names.length === 0 && projects.status === "loading" && <div className="rnd-tree-hint">loading…</div>}
                {names.length === 0 && projects.status === "ok" && <div className="rnd-tree-hint">no projects</div>}
            </div>
            {projects.error && !projects.data && <div className="rnd-tree-err">{projects.error}</div>}
            {index.error && !index.data && <div className="rnd-tree-err">{index.error}</div>}
        </aside>
    );
}

interface BranchProps {
    paneId: string;
    project: string;
    entry: { jobs: RundeckJob[]; error: string | null } | undefined;
    indexLoading: boolean;
    activeProject: string;
    activeGroup: string | null;
    isOpen: (project: string, path: string | null) => boolean;
}

function ProjectBranch({ paneId, project, entry, indexLoading, activeProject, activeGroup, isOpen }: BranchProps) {
    const tree = useMemo(() => buildGroupTree(entry?.jobs ?? []), [entry]);
    const open = isOpen(project, null);
    const selected = project === activeProject && activeGroup === null;
    const hasChildren = tree.children.length > 0 || tree.ungrouped > 0;

    return (
        <div className="rnd-tree-group" role="treeitem" aria-expanded={hasChildren ? open : undefined} aria-selected={selected}>
            <div className={`rnd-tree-row${selected ? " active" : ""}`}>
                <Chevron paneId={paneId} project={project} path={null} open={open} visible={hasChildren} />
                <button type="button" className="rnd-tree-select" onClick={() => select(paneId, project, null)} title={project}>
                    <span className="rnd-tree-ic">
                        <IconFolder size={11} />
                    </span>
                    <span className="rnd-tree-name">{project}</span>
                    {entry && <span className="rnd-tree-n">{tree.total}</span>}
                </button>
            </div>
            {open && (
                <div className="rnd-tree-children" role="group">
                    {tree.children.map((node) => (
                        <GroupBranch
                            key={node.path}
                            paneId={paneId}
                            project={project}
                            node={node}
                            activeProject={activeProject}
                            activeGroup={activeGroup}
                            isOpen={isOpen}
                        />
                    ))}
                    {tree.ungrouped > 0 && tree.children.length > 0 && (
                        <div className="rnd-tree-hint indent">
                            (no group) <span className="rnd-tree-n">{tree.ungrouped}</span>
                        </div>
                    )}
                    {!entry && indexLoading && <div className="rnd-tree-hint indent">loading…</div>}
                    {entry?.error && <div className="rnd-tree-hint indent danger">{entry.error}</div>}
                </div>
            )}
        </div>
    );
}

function GroupBranch({
    paneId,
    project,
    node,
    activeProject,
    activeGroup,
    isOpen,
}: {
    paneId: string;
    project: string;
    node: GroupNode;
    activeProject: string;
    activeGroup: string | null;
    isOpen: (project: string, path: string | null) => boolean;
}) {
    const open = isOpen(project, node.path);
    const selected = project === activeProject && activeGroup === node.path;
    const hasChildren = node.children.length > 0;
    return (
        <div className="rnd-tree-group" role="treeitem" aria-expanded={hasChildren ? open : undefined} aria-selected={selected}>
            <div className={`rnd-tree-leaf${selected ? " active" : ""}`}>
                <Chevron paneId={paneId} project={project} path={node.path} open={open} visible={hasChildren} />
                <button
                    type="button"
                    className="rnd-tree-select"
                    onClick={() => select(paneId, project, node.path)}
                    title={`${project} · ${node.path}/`}>
                    <span className="rnd-tree-leaf-name">{node.name}/</span>
                    <span className="rnd-tree-n">{node.count}</span>
                </button>
            </div>
            {open && hasChildren && (
                <div className="rnd-tree-children" role="group">
                    {node.children.map((child) => (
                        <GroupBranch
                            key={child.path}
                            paneId={paneId}
                            project={project}
                            node={child}
                            activeProject={activeProject}
                            activeGroup={activeGroup}
                            isOpen={isOpen}
                        />
                    ))}
                </div>
            )}
        </div>
    );
}

function Chevron({
    paneId,
    project,
    path,
    open,
    visible,
}: {
    paneId: string;
    project: string;
    path: string | null;
    open: boolean;
    visible: boolean;
}) {
    if (!visible) return <span className="rnd-tree-chev" />;
    return (
        <button
            type="button"
            className="rnd-tree-chev"
            aria-label={open ? "Collapse" : "Expand"}
            onClick={() => cmd.setTreeOpen(paneId, cmd.treeKey(project, path), !open)}>
            <IconChevron size={9} className={`rnd-tree-chev-ic${open ? " open" : ""}`} />
        </button>
    );
}

function select(paneId: string, project: string, path: string | null): void {
    cmd.setTreeOpen(paneId, cmd.treeKey(project, path), true);
    cmd.selectRundeckGroup(paneId, project, path);
}
