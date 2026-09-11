import { git, type GitWorktree } from "../api/git";
import { performanceTelemetry } from "../lib/performance";
import { loadProjectConfig, type ProjectConfigLoadResult } from "../projectConfig";
import { subscribe as subscribeBus } from "../state/bus";
import {
    ProjectControllerRegistry,
    type ProjectController,
    type ProjectControllerServices,
    type ProjectControllerSnapshot,
} from "../workbench/projectController";

export const PROJECT_CONTROLLER_RUNTIME_LIMITS = Object.freeze({
    defaultMaxRoots: 64,
    hardMaxRoots: 128,
    maxRootCandidates: 512,
    maxRootLength: 4_096,
});

export type ActiveProjectControllerSnapshot = ProjectControllerSnapshot<ProjectConfigLoadResult, GitWorktree>;

type Listener = () => void;
type ProjectChangeListener = (repo: string, paths?: readonly string[]) => void;

export interface ProjectControllerRuntimeServices {
    readonly loadConfig: (cwd: string) => Promise<ProjectConfigLoadResult>;
    readonly loadWorktrees: (cwd: string) => Promise<readonly GitWorktree[]>;
    readonly newWatchLeaseToken: () => string;
    readonly watchStart: (cwd: string, token: string) => void | PromiseLike<void>;
    readonly watchStop: (token: string) => void | PromiseLike<void>;
    readonly subscribeFsChanged: (listener: ProjectChangeListener) => () => void;
    readonly subscribeGitRefresh: (listener: ProjectChangeListener) => () => void;
}

export interface ProjectControllerRuntimeOptions {
    readonly maxRoots?: number;
}

interface RootOwner {
    readonly registry: ProjectControllerRegistry<ProjectConfigLoadResult, GitWorktree>;
    readonly controller: ProjectController<ProjectConfigLoadResult, GitWorktree>;
    readonly release: () => void;
    readonly unsubscribe: () => void;
}

const WINDOWS_ABSOLUTE_PATH = /^(?:[a-zA-Z]:[\\/]|\\\\[^\\/]+[\\/][^\\/]+)/u;

function isBoundedAbsoluteRoot(value: unknown): value is string {
    if (typeof value !== "string" || value.length === 0 || value.length > PROJECT_CONTROLLER_RUNTIME_LIMITS.maxRootLength) return false;
    if (!(value.startsWith("/") || WINDOWS_ABSOLUTE_PATH.test(value))) return false;
    for (let index = 0; index < value.length; index += 1) {
        const code = value.charCodeAt(index);
        if (code <= 0x1f || code === 0x7f) return false;
    }
    return true;
}

function requireServices(services: ProjectControllerRuntimeServices): ProjectControllerRuntimeServices {
    if (typeof services !== "object" || services === null) throw new TypeError("project controller runtime services are required");
    const keys = [
        "loadConfig",
        "loadWorktrees",
        "newWatchLeaseToken",
        "watchStart",
        "watchStop",
        "subscribeFsChanged",
        "subscribeGitRefresh",
    ] as const;
    for (const key of keys) {
        if (typeof services[key] !== "function") throw new TypeError(`project controller runtime service ${key} must be a function`);
    }
    return Object.freeze({ ...services });
}

function requireMaxRoots(value: number | undefined): number {
    const resolved = value ?? PROJECT_CONTROLLER_RUNTIME_LIMITS.defaultMaxRoots;
    if (!Number.isSafeInteger(resolved) || resolved < 1 || resolved > PROJECT_CONTROLLER_RUNTIME_LIMITS.hardMaxRoots) {
        throw new RangeError(`project controller runtime maxRoots must be from 1 through ${PROJECT_CONTROLLER_RUNTIME_LIMITS.hardMaxRoots}`);
    }
    return resolved;
}

function invokeService<Result>(operation: () => Result | PromiseLike<Result>): Promise<Result> {
    // Starting from a resolved promise converts synchronous adapter failures
    // into rejections which ProjectController can publish without escaping an
    // event callback or creating an unhandled rejection.
    return Promise.resolve().then(operation);
}

const productionServices: ProjectControllerRuntimeServices = Object.freeze({
    loadConfig: (cwd: string) => loadProjectConfig(cwd),
    loadWorktrees: (cwd: string) => git.worktrees(cwd),
    newWatchLeaseToken: () => globalThis.crypto.randomUUID(),
    watchStart: (cwd: string, token: string) => git.watchStart(cwd, token),
    watchStop: (token: string) => git.watchStop(token),
    subscribeFsChanged: (listener: ProjectChangeListener) => subscribeBus("fs-changed", (event) => listener(event.repo, event.paths)),
    subscribeGitRefresh: (listener: ProjectChangeListener) => subscribeBus("git-refresh", (event) => listener(event.repo)),
});

/**
 * Frontend owner for every open local project's config, worktrees, and native
 * watcher lease. Importing the runtime is side-effect free; callers explicitly
 * start it and reconcile the current set of project roots.
 */
export class ProjectControllerRuntime {
    private readonly listeners = new Set<Listener>();
    private readonly roots = new Map<string, RootOwner>();
    private readonly services: ProjectControllerRuntimeServices;
    private readonly controllerServices: ProjectControllerServices<ProjectConfigLoadResult, GitWorktree>;
    private readonly maxRoots: number;
    private subscriptions: readonly [() => void, () => void] | null = null;
    private subscriptionToken: object | null = null;
    private desiredRoots: readonly string[] = Object.freeze([]);
    private desiredActiveRoot: string | null = null;
    private activeRoot: string | null = null;
    private disposed = false;

    constructor(services: ProjectControllerRuntimeServices = productionServices, options: ProjectControllerRuntimeOptions = {}) {
        this.services = requireServices(services);
        this.maxRoots = requireMaxRoots(options.maxRoots);
        this.controllerServices = Object.freeze({
            loadConfig: (cwd: string) => invokeService(() => this.services.loadConfig(cwd)),
            loadWorktrees: (cwd: string) => invokeService(() => this.services.loadWorktrees(cwd)),
            newWatchLeaseToken: () => this.services.newWatchLeaseToken(),
            watchStart: (cwd: string, token: string) => invokeService(() => this.services.watchStart(cwd, token)),
            watchStop: (token: string) => invokeService(() => this.services.watchStop(token)),
        });
    }

    get isStarted(): boolean {
        return this.subscriptions !== null;
    }

    subscribe = (listener: Listener): (() => void) => {
        if (typeof listener !== "function" || this.disposed) return () => {};
        this.listeners.add(listener);
        let subscribed = true;
        return () => {
            if (!subscribed) return;
            subscribed = false;
            this.listeners.delete(listener);
        };
    };

    /** Stable getter suitable for React's useSyncExternalStore. */
    getActiveSnapshot = (): ActiveProjectControllerSnapshot | null => {
        if (!this.activeRoot) return null;
        return this.roots.get(this.activeRoot)?.controller.getSnapshot() ?? null;
    };

    /** Install exactly one listener for each project invalidation event. */
    start = (): Promise<void> => {
        if (this.disposed || this.subscriptions) return Promise.resolve();

        let stopFs: (() => void) | undefined;
        const token = Object.freeze({});
        try {
            stopFs = this.services.subscribeFsChanged((repo, paths) => this.handleProjectChange(token, repo, paths));
            if (typeof stopFs !== "function") throw new TypeError("fs-changed subscription must return a disposer");
            const stopGit = this.services.subscribeGitRefresh((repo) => this.handleProjectChange(token, repo));
            if (typeof stopGit !== "function") throw new TypeError("git-refresh subscription must return a disposer");
            this.subscriptions = Object.freeze([stopFs, stopGit]);
            this.subscriptionToken = token;
        } catch {
            if (stopFs) this.safeTeardown(stopFs);
            performanceTelemetry.incrementCounter("project.runtime.start.failures");
            return Promise.resolve();
        }

        return this.applyDesiredRoots();
    };

    /**
     * Reconcile a deterministic first-seen set. Invalid, duplicate, and excess
     * roots are ignored before any ownership changes; work is capped even if a
     * corrupted persisted state contains an enormous candidate array.
     */
    reconcile = (rootsInput: readonly string[], activeRootInput: string | null): Promise<void> => {
        if (this.disposed) return Promise.resolve();
        const roots = this.boundRoots(rootsInput);
        this.desiredRoots = roots;
        this.desiredActiveRoot = activeRootInput !== null && roots.includes(activeRootInput) ? activeRootInput : null;
        return this.isStarted ? this.applyDesiredRoots() : Promise.resolve();
    };

    /** Refresh the requested owned root, or the active root when omitted. */
    refresh = (root: string | undefined = this.activeRoot ?? undefined): Promise<void> => {
        if (!this.isStarted || !root) return Promise.resolve();
        const owner = this.roots.get(root);
        return owner ? this.contain(owner.controller.refresh()) : Promise.resolve();
    };

    /** Stop event delivery and synchronously release every watcher/controller owner. */
    stop = (): void => {
        const subscriptions = this.subscriptions;
        this.subscriptions = null;
        this.subscriptionToken = null;
        if (subscriptions) {
            for (const unsubscribe of subscriptions) this.safeTeardown(unsubscribe);
        }

        const changed = this.roots.size > 0 || this.activeRoot !== null;
        for (const root of Array.from(this.roots.keys())) this.removeRoot(root);
        this.activeRoot = null;
        this.desiredRoots = Object.freeze([]);
        this.desiredActiveRoot = null;
        if (changed) this.notify();
    };

    dispose = (): void => {
        if (this.disposed) return;
        this.stop();
        this.disposed = true;
        this.listeners.clear();
    };

    private handleProjectChange(token: object, repo: string, paths?: readonly string[]): void {
        if (!this.isStarted || this.subscriptionToken !== token) return;
        if (paths?.length && !paths.some((path) => path === "sikemux.json" || path === ".git" || path.startsWith(".git/"))) return;
        if (repo) {
            const owner = this.roots.get(repo);
            if (owner) {
                void this.contain(owner.controller.refresh());
                return;
            }
        }
        // Native watchers may report a canonicalized path while persisted/open
        // roots retain a symlink or other alias. An unknown non-empty key cannot
        // be routed safely, so invalidate every bounded owner instead of leaving
        // an aliased project stale.
        for (const owner of this.roots.values()) void this.contain(owner.controller.refresh());
    }

    private boundRoots(input: readonly string[]): readonly string[] {
        if (!Array.isArray(input)) {
            performanceTelemetry.incrementCounter("project.runtime.roots.rejected");
            return Object.freeze([]);
        }

        const roots: string[] = [];
        const seen = new Set<string>();
        const candidateCount = Math.min(input.length, PROJECT_CONTROLLER_RUNTIME_LIMITS.maxRootCandidates);
        let rejected = input.length > candidateCount ? 1 : 0;
        for (let index = 0; index < candidateCount; index += 1) {
            const root: unknown = input[index];
            if (!isBoundedAbsoluteRoot(root) || seen.has(root)) {
                rejected += 1;
                continue;
            }
            seen.add(root);
            if (roots.length >= this.maxRoots) {
                rejected += 1;
                continue;
            }
            roots.push(root);
        }
        if (rejected > 0) performanceTelemetry.incrementCounter("project.runtime.roots.rejected", rejected);
        return Object.freeze(roots);
    }

    private applyDesiredRoots(): Promise<void> {
        if (!this.isStarted || this.disposed) return Promise.resolve();
        const desired = new Set(this.desiredRoots);
        let changed = false;

        for (const root of Array.from(this.roots.keys())) {
            if (desired.has(root)) continue;
            this.removeRoot(root);
            changed = true;
        }

        const ready: Promise<void>[] = [];
        for (const root of this.desiredRoots) {
            if (this.roots.has(root)) continue;
            const created = this.createRoot(root);
            if (!created) continue;
            this.roots.set(root, created.owner);
            ready.push(created.ready);
            changed = true;
        }

        const nextActiveRoot = this.desiredActiveRoot && this.roots.has(this.desiredActiveRoot) ? this.desiredActiveRoot : null;
        if (this.activeRoot !== nextActiveRoot) {
            this.activeRoot = nextActiveRoot;
            changed = true;
        }
        if (changed) this.notify();
        return ready.length === 0 ? Promise.resolve() : Promise.all(ready).then(() => undefined);
    }

    private createRoot(root: string): { readonly owner: RootOwner; readonly ready: Promise<void> } | null {
        const registry = new ProjectControllerRegistry<ProjectConfigLoadResult, GitWorktree>(this.controllerServices);
        try {
            const lease = registry.acquire(root);
            const controller = lease.controller;
            const unsubscribe = controller.subscribe(() => {
                if (this.isStarted && this.activeRoot === root && this.roots.get(root)?.controller === controller) this.notify();
            });
            return {
                owner: { registry, controller, release: lease.release, unsubscribe },
                ready: this.contain(lease.ready),
            };
        } catch {
            this.safeTeardown(() => registry.dispose());
            performanceTelemetry.incrementCounter("project.runtime.acquire.failures");
            return null;
        }
    }

    private removeRoot(root: string): void {
        const owner = this.roots.get(root);
        if (!owner) return;
        this.roots.delete(root);
        this.safeTeardown(owner.unsubscribe);
        this.safeTeardown(owner.release);
        // ProjectControllerRegistry has no per-key removal API. One registry per
        // root lets disposal clear its internal map and prevents lifetime growth.
        this.safeTeardown(() => owner.registry.dispose());
    }

    private contain(promise: Promise<void>): Promise<void> {
        return promise.catch(() => {
            performanceTelemetry.incrementCounter("project.runtime.async.failures");
        });
    }

    private safeTeardown(teardown: () => void): void {
        try {
            teardown();
        } catch {
            performanceTelemetry.incrementCounter("project.runtime.teardown.failures");
        }
    }

    private notify(): void {
        for (const listener of this.listeners) {
            try {
                listener();
            } catch {
                performanceTelemetry.incrementCounter("project.runtime.listener.failures");
            }
        }
    }
}

export const projectControllerRuntime = new ProjectControllerRuntime();
