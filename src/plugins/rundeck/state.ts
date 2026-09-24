import { create } from "zustand";
import { closeCorePalettes, onPaneClosed, openSurface } from "../../plugin-api/host";
import { definePluginSettings } from "../../plugin-api/settings";
import { RUNDECK_DEPLOY, RUNDECK_PLUGIN_ID } from "./kinds";
import { DEFAULT_BRANCH_OPTIONS, DEFAULT_PROD_ENVS, basenameOf } from "./shape";

/** A Rundeck job as the views address it, plus the local checkout linked to it, if any. */
export interface JobRef {
    project: string;
    jobId: string;
    name: string;
    group: string | null;
    repoPath?: string;
}

export type RundeckLevel =
    | { kind: "matrix" }
    | ({ kind: "service" } & JobRef)
    | ({ kind: "deploy"; branch?: string; options?: Record<string, string> } & JobRef)
    | ({ kind: "execution"; executionId: number } & JobRef);

export interface RundeckView {
    stack: RundeckLevel[];
}

export interface DeployTarget {
    project: string;
    jobId: string;
}

export interface RundeckSettings {
    activeProject: string;
    activeGroup: string | null;
    prodEnvs: string[];
    branchOptions: string[];
    /** The job picked for each local project folder. */
    deployTargets: Record<string, DeployTarget>;
    treeHidden: boolean;
}

const isRecord = (value: unknown): value is Record<string, unknown> => !!value && typeof value === "object" && !Array.isArray(value);

const stringList = (value: unknown, fallback: string[]): string[] => {
    if (!Array.isArray(value)) return fallback;
    return value.filter((entry): entry is string => typeof entry === "string");
};

function decodeSettings(saved: unknown): RundeckSettings {
    const raw = isRecord(saved) ? saved : {};
    const deployTargets: Record<string, DeployTarget> = {};
    for (const [cwd, target] of Object.entries(isRecord(raw.deployTargets) ? raw.deployTargets : {})) {
        if (isRecord(target) && typeof target.project === "string" && typeof target.jobId === "string") {
            deployTargets[cwd] = { project: target.project, jobId: target.jobId };
        }
    }
    return {
        activeProject: typeof raw.activeProject === "string" ? raw.activeProject : "",
        activeGroup: typeof raw.activeGroup === "string" && raw.activeGroup ? raw.activeGroup : null,
        prodEnvs: stringList(raw.prodEnvs, DEFAULT_PROD_ENVS),
        branchOptions: stringList(raw.branchOptions, DEFAULT_BRANCH_OPTIONS),
        deployTargets,
        treeHidden: raw.treeHidden === true,
    };
}

export const rundeckSettings = definePluginSettings(RUNDECK_PLUGIN_ID, decodeSettings);

export function updateRundeckSettings(patch: Partial<RundeckSettings>): void {
    rundeckSettings.update((settings) => ({ ...settings, ...patch }));
}

export function setDeployTarget(projectCwd: string, target: DeployTarget): void {
    rundeckSettings.update((settings) => ({ ...settings, deployTargets: { ...settings.deployTargets, [projectCwd]: target } }));
}

/** The local checkout for a job: the project in front when its folder is named after the job, else the folder that picked this job. */
export function linkedRepoPath(job: { jobId: string; name: string }, activeCwd: string | null): string | undefined {
    if (activeCwd && basenameOf(activeCwd) === job.name) return activeCwd;
    const targets = rundeckSettings.get().deployTargets;
    return Object.keys(targets).find((cwd) => targets[cwd].jobId === job.jobId);
}

export const TREE_MIN_WIDTH = 140;
export const TREE_MAX_WIDTH = 360;

interface RundeckRuntime {
    views: Record<string, RundeckView>;
    jobPaletteOpen: boolean;
    /** Folder open/closed choices per pane, keyed by `treeKey`. Absent means "follow the selection". */
    treeOpen: Record<string, Record<string, boolean>>;
    treeWidth: number;
}

export const useRundeck = create<RundeckRuntime>()(() => ({ views: {}, jobPaletteOpen: false, treeOpen: {}, treeWidth: 196 }));

onPaneClosed((paneId) => {
    const state = useRundeck.getState();
    if (!(paneId in state.views) && !(paneId in state.treeOpen)) return;
    useRundeck.setState((current) => {
        const views = { ...current.views };
        const treeOpen = { ...current.treeOpen };
        delete views[paneId];
        delete treeOpen[paneId];
        return { views, treeOpen };
    });
});

export const treeKey = (project: string, path: string | null): string => `${project}\u0000${path ?? ""}`;

export function setTreeOpen(paneId: string, key: string, open: boolean): void {
    useRundeck.setState((state) => ({ treeOpen: { ...state.treeOpen, [paneId]: { ...state.treeOpen[paneId], [key]: open } } }));
}

export function setTreeWidth(width: number): void {
    useRundeck.setState({ treeWidth: Math.round(Math.max(TREE_MIN_WIDTH, Math.min(TREE_MAX_WIDTH, width))) });
}

const HOME: RundeckView = { stack: [{ kind: "matrix" }] };

export const rundeckView = (paneId: string): RundeckView => useRundeck.getState().views[paneId] ?? HOME;

export function useRundeckView(paneId: string): RundeckView {
    return useRundeck((state) => state.views[paneId] ?? HOME);
}

function setStack(paneId: string, stack: RundeckLevel[]): void {
    useRundeck.setState((state) => ({ views: { ...state.views, [paneId]: { stack } } }));
}

export function rundeckPush(paneId: string, level: RundeckLevel): void {
    setStack(paneId, [...rundeckView(paneId).stack, level]);
}

export function rundeckReplace(paneId: string, level: RundeckLevel): void {
    setStack(paneId, [...rundeckView(paneId).stack.slice(0, -1), level]);
}

export function rundeckPop(paneId: string): void {
    const { stack } = rundeckView(paneId);
    if (stack.length > 1) setStack(paneId, stack.slice(0, -1));
}

export function rundeckPopTo(paneId: string, index: number): void {
    const { stack } = rundeckView(paneId);
    const target = Math.max(0, Math.min(index, stack.length - 1));
    setStack(paneId, stack.slice(0, target + 1));
}

export function rundeckHome(paneId: string): void {
    setStack(paneId, HOME.stack);
}

export function selectRundeckGroup(paneId: string, project: string, group: string | null = null): void {
    updateRundeckSettings({ activeProject: project, activeGroup: group });
    rundeckHome(paneId);
}

export function openRundeckJobPalette(): void {
    closeCorePalettes();
    useRundeck.setState({ jobPaletteOpen: true });
}

export function closeRundeckJobPalette(): void {
    useRundeck.setState({ jobPaletteOpen: false });
}

export function toggleRundeckJobPalette(): void {
    if (useRundeck.getState().jobPaletteOpen) closeRundeckJobPalette();
    else openRundeckJobPalette();
}

export const openRundeckSession = (): void => {
    openSurface(RUNDECK_DEPLOY);
};

/**
 * Opens a job, and its run form when a branch is given. `push` keeps the pane's
 * history underneath; otherwise the pane starts over from the job list.
 */
export function openRundeckJob(job: JobRef, options: { paneId?: string | null; branch?: string; push?: boolean } = {}): void {
    const paneId = options.paneId ?? openSurface(RUNDECK_DEPLOY);
    if (!paneId) return;
    const settings = rundeckSettings.get();
    if (settings.activeProject !== job.project) updateRundeckSettings({ activeProject: job.project, activeGroup: null });
    const service: RundeckLevel = { kind: "service", ...job };
    const deploy: RundeckLevel[] = options.branch !== undefined ? [{ kind: "deploy", ...job, branch: options.branch }] : [];
    const base = options.push ? rundeckView(paneId).stack : HOME.stack;
    setStack(paneId, [...base, service, ...deploy]);
}
