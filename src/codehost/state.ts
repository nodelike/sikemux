import { definePaneViews } from "../plugin-api/paneViews";
import { definePluginSettings, type PluginSettings } from "../plugin-api/settings";
import { coreCall } from "../api/core";
import type { CodeHostChoice } from "../api/generated/wire";
import { setGitView } from "../state/commands";
import { getState, setState } from "../state/store";
import { swallow } from "../state/toast";
import type { RepoRef } from "./types";
import { isRecord } from "../lib/guards";

export const STATUS_FILTERS = ["all", "in_progress", "queued", "success", "failure", "cancelled"] as const;
export type StatusFilter = (typeof STATUS_FILTERS)[number];

export const SECTIONS = ["pulls", "actions", "issues", "releases", "inbox"] as const;
export type Section = (typeof SECTIONS)[number];

/** The sections the project tree lists under Git, which open as screens of their own rather than as tabs in the Git pane. */
export const TREE_SECTIONS = ["pulls", "issues", "actions", "releases"] as const satisfies readonly Section[];
export type TreeSection = (typeof TREE_SECTIONS)[number];
export const isTreeSection = (area: string): area is TreeSection => (TREE_SECTIONS as readonly string[]).includes(area);

export interface HostSettings {
    pinned: string[];
    followBranch: boolean;
}

const isSlug = (value: unknown): value is string => typeof value === "string" && /^[\w.-]+\/[\w.-]+$/u.test(value);

function decodeSettings(saved: unknown): HostSettings {
    const raw = isRecord(saved) ? saved : {};
    return {
        pinned: Array.isArray(raw.pinned) ? [...new Set(raw.pinned.filter(isSlug))] : [],
        followBranch: raw.followBranch !== false,
    };
}

const settingsByHost = new Map<string, PluginSettings<HostSettings>>();

/** Kept with the host plugin's own settings, so what someone pinned stays with the host it was pinned on. */
export function hostSettings(provider: string): PluginSettings<HostSettings> {
    let settings = settingsByHost.get(provider);
    if (!settings) {
        settings = definePluginSettings(provider, decodeSettings);
        settingsByHost.set(provider, settings);
    }
    return settings;
}

export function togglePinned(provider: string, slug: string): void {
    hostSettings(provider).update((settings) => ({
        ...settings,
        pinned: settings.pinned.includes(slug) ? settings.pinned.filter((kept) => kept !== slug) : [...settings.pinned, slug],
    }));
}

/**
 * Picks the repository or account `provider` shows for the project at `cwd`, over what its remote and the host's accounts
 * say, or with null goes back to them. The core keeps the pick, so Auto and every device use the same one.
 */
function pickCodeHost(provider: string, cwd: string, change: CodeHostChoice): void {
    const state = getState();
    const current = state.codeHosts[cwd] ?? {};
    const choice = Object.fromEntries(Object.entries({ ...current[provider], ...change }).filter(([, value]) => value)) as CodeHostChoice;
    const { [provider]: _replaced, ...others } = current;
    const next = choice.repo || choice.account ? { ...others, [provider]: choice } : others;
    setState({ codeHosts: { ...state.codeHosts, [cwd]: next } });
    const project = Object.values(state.sessions).find((session) => session.kind === "project" && session.cwd === cwd);
    if (project) void coreCall("updateProject", { id: project.id, patch: { codeHosts: next } }).catch(swallow("pick the project's code host"));
}

export function setProjectRepo(provider: string, cwd: string, slug: string | null): void {
    pickCodeHost(provider, cwd, { repo: slug ?? undefined });
}

export function setProjectAccount(provider: string, cwd: string, account: string | null): void {
    pickCodeHost(provider, cwd, { account: account ?? undefined });
}

/** Every project that picked an account forgets it once it signs out, and goes back to finding one. */
export function forgetAccount(provider: string, account: string): void {
    for (const [cwd, hosts] of Object.entries(getState().codeHosts)) if (hosts[provider]?.account === account) setProjectAccount(provider, cwd, null);
}

export function setFollowBranch(provider: string, followBranch: boolean): void {
    hostSettings(provider).update((settings) => ({ ...settings, followBranch }));
}

/** What one Git pane shows of its host: the open section is kept in the pane's own view, everything else here. */
export interface HostView {
    /** The pull request or issue open in this pane, by number. */
    item: number | null;
    /** `open`, `closed` or `all`, for each list on its own. */
    pullState: string;
    issueState: string;
    workflowId: string | null;
    statusFilter: StatusFilter;
    /** A branch typed into the filter, which wins over following the project's branch. */
    branch: string | null;
    run: string | null;
    job: string | null;
    page: number;
    dispatching: string | null;
    composing: "pull" | "issue" | null;
    /** The pull request a run was opened from, which its Back returns to. */
    runFrom: number | null;
    /** Open the run's first failed job once its jobs are read, as a check does. */
    pickFailed: boolean;
    /** What the right column shows of an open run. */
    runTab: RunTab;
}

export type RunTab = "summary" | "logs";

const FRESH: HostView = {
    item: null,
    pullState: "all",
    issueState: "open",
    workflowId: null,
    statusFilter: "all",
    branch: null,
    run: null,
    job: null,
    page: 1,
    dispatching: null,
    composing: null,
    runFrom: null,
    pickFailed: false,
    runTab: "summary",
};

const views = definePaneViews(FRESH);

export const useHostView = views.use;
export const viewOf = views.get;
export const updateView = views.update;

/** Everything a pane had open belongs to the repository it was showing, so a different one starts clean. */
export function resetView(paneId: string): void {
    updateView(paneId, FRESH);
}

export function filterBy(paneId: string, patch: Pick<Partial<HostView>, "workflowId" | "statusFilter" | "branch">): void {
    updateView(paneId, { ...patch, item: null, composing: null, page: 1, run: null, job: null });
}

export function showItem(paneId: string, number: number | null): void {
    updateView(paneId, { item: number, composing: null });
}

export function compose(paneId: string, composing: "pull" | "issue" | null): void {
    updateView(paneId, { composing, item: null });
}

export function setListState(paneId: string, list: "pulls" | "issues", state: string): void {
    updateView(paneId, { [list === "pulls" ? "pullState" : "issueState"]: state, item: null, page: 1 });
}

/** A check on a pull request opens its run in the same pane, at its first failed job, with Back leading to the pull request. */
export function openRunFrom(paneId: string, runId: string, pull: number | null = null): void {
    setGitView(paneId, { area: "actions" });
    updateView(paneId, { item: null, composing: null, run: runId, job: null, runFrom: pull, pickFailed: true, runTab: "summary" });
}

/** Leaves a run for wherever it was opened from: the pull request whose check it was, or the runs list. */
export function leaveRun(paneId: string): void {
    const from = viewOf(paneId).runFrom;
    if (from === null) {
        closeRun(paneId);
        return;
    }
    setGitView(paneId, { area: "pulls" });
    updateView(paneId, { run: null, job: null, runFrom: null, item: from });
}

export function showRun(paneId: string, runId: string): void {
    updateView(paneId, { run: runId, job: null, runFrom: null, pickFailed: false, runTab: "summary" });
}

/** Opens a job of the open run on the right, at its log. */
export function showJob(paneId: string, jobId: string): void {
    updateView(paneId, { job: jobId, runTab: "logs" });
}

export function closeRun(paneId: string): void {
    updateView(paneId, { run: null, job: null, runFrom: null });
}

export function refOf(provider: string, slug: string): RepoRef | null {
    const [owner, name, ...rest] = slug.split("/");
    if (!owner || !name || rest.length > 0) return null;
    return { provider, owner, name };
}

export function slugOf(repo: Pick<RepoRef, "owner" | "name">): string {
    return `${repo.owner}/${repo.name}`;
}

export function sameRepo(a: RepoRef | null, b: RepoRef | null): boolean {
    return !!a && !!b && a.provider === b.provider && slugOf(a) === slugOf(b);
}
