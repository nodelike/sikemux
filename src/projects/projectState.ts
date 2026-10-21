import { git, type GitWorktree } from "../api/git";
import { subscribe as subscribeBus } from "../state/bus";
import { loadProjectConfig, type ProjectConfigLoadResult } from "./projectConfig";

export interface ProjectSnapshot {
    readonly cwd: string;
    readonly config: ProjectConfigLoadResult | null;
    readonly worktrees: readonly GitWorktree[];
}

const projects = new Map<string, ProjectSnapshot>();
const inFlight = new Map<string, Promise<void>>();
const queued = new Set<string>();
const listeners = new Set<() => void>();
let activeRoot: string | null = null;

function notify(): void {
    for (const listener of listeners) listener();
}

async function load(root: string): Promise<void> {
    do {
        queued.delete(root);
        const [config, worktrees] = await Promise.allSettled([loadProjectConfig(root), git.worktrees(root)]);
        const current = projects.get(root);
        if (!current) return;
        projects.set(root, {
            cwd: root,
            config: config.status === "fulfilled" ? config.value : current.config,
            worktrees: worktrees.status === "fulfilled" ? worktrees.value : current.worktrees,
        });
        if (root === activeRoot) notify();
    } while (queued.has(root));
}

export function refreshProject(root: string): Promise<void> {
    if (!projects.has(root)) return Promise.resolve();
    const running = inFlight.get(root);
    if (running) {
        queued.add(root);
        return running;
    }
    const started = load(root).finally(() => inFlight.delete(root));
    inFlight.set(root, started);
    return started;
}

/** Keeps the config and worktrees of every open project loaded; `active` is the one in front. */
export function setOpenProjects(roots: readonly string[], active: string | null): void {
    const open = new Set(roots);
    for (const root of projects.keys()) if (!open.has(root)) projects.delete(root);
    for (const root of open) {
        if (projects.has(root)) continue;
        projects.set(root, { cwd: root, config: null, worktrees: [] });
        void refreshProject(root);
    }
    const nextActive = active && open.has(active) ? active : null;
    if (nextActive !== activeRoot) {
        activeRoot = nextActive;
        notify();
    }
}

function touchesProject(paths: readonly string[] | undefined): boolean {
    return !paths?.length || paths.some((path) => path === "sikemux.json" || path === ".git" || path.startsWith(".git/"));
}

function onProjectChanged(repo: string): void {
    // A watcher may name a project by its resolved path while the open one goes by a symlink, so an unknown one refreshes them all.
    if (projects.has(repo)) void refreshProject(repo);
    else for (const root of projects.keys()) void refreshProject(root);
}

export function watchProjects(): () => void {
    const stopFs = subscribeBus("fs-changed", (event) => {
        if (touchesProject(event.paths)) onProjectChanged(event.repo);
    });
    const stopGit = subscribeBus("git-refresh", (event) => onProjectChanged(event.repo));
    return () => {
        stopFs();
        stopGit();
    };
}

export function subscribeProjects(listener: () => void): () => void {
    listeners.add(listener);
    return () => listeners.delete(listener);
}

export function activeProject(): ProjectSnapshot | null {
    return activeRoot ? (projects.get(activeRoot) ?? null) : null;
}
