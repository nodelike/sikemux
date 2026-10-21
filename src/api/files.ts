import { invokeCommand } from "./invoke";

export interface ProjectFilesSnapshot {
    readonly scanId: number;
    readonly files: string[];
}

interface CachedSnapshot {
    readonly generation: number;
    readonly snapshot: ProjectFilesSnapshot;
}

interface InflightSnapshot {
    readonly generation: number;
    readonly promise: Promise<ProjectFilesSnapshot>;
}

const inflight = new Map<string, InflightSnapshot>();
const cache = new Map<string, CachedSnapshot>();
const repoGenerations = new Map<string, number>();
export const MAX_FRONTEND_FILE_SNAPSHOTS = 32;
let invalidationSequence = 0;
let globalGeneration = 0;

function generationFor(repo: string): number {
    return Math.max(globalGeneration, repoGenerations.get(repo) ?? 0);
}

function touchCache(repo: string, entry: CachedSnapshot): void {
    cache.delete(repo);
    cache.set(repo, entry);
    while (cache.size > MAX_FRONTEND_FILE_SNAPSHOTS) {
        const oldest = cache.keys().next().value as string | undefined;
        if (oldest === undefined) break;
        cache.delete(oldest);
        if (!inflight.has(oldest)) repoGenerations.delete(oldest);
    }
}

function snapshot(repo: string): Promise<ProjectFilesSnapshot> {
    const generation = generationFor(repo);
    const hit = cache.get(repo);
    if (hit?.generation === generation) {
        touchCache(repo, hit);
        return Promise.resolve(hit.snapshot);
    }

    const pending = inflight.get(repo);
    if (pending?.generation === generation) return pending.promise;

    const promise = invokeCommand<ProjectFilesSnapshot>("list_project_files_snapshot", { repo })
        .then((incoming) => {
            const previous = cache.get(repo)?.snapshot;
            // scanId is process-monotonic. Never let a late request replace a
            // newer snapshot, and preserve array identity when nothing changed.
            const resolved =
                previous && previous.scanId > incoming.scanId
                    ? previous
                    : previous?.scanId === incoming.scanId
                      ? { scanId: incoming.scanId, files: previous.files }
                      : incoming;
            if (generationFor(repo) === generation) touchCache(repo, { generation, snapshot: resolved });
            return resolved;
        })
        .finally(() => {
            if (inflight.get(repo)?.promise === promise) inflight.delete(repo);
        });
    inflight.set(repo, { generation, promise });
    return promise;
}

export const filesApi = {
    snapshot,
    list: (repo: string): Promise<string[]> => snapshot(repo).then((result) => result.files),
    invalidate: (repo?: string) => {
        const generation = ++invalidationSequence;
        if (repo) repoGenerations.set(repo, generation);
        else globalGeneration = generation;
    },
    evict: (repo: string) => {
        cache.delete(repo);
        if (!inflight.has(repo)) repoGenerations.delete(repo);
    },
    stats: () => ({ cacheEntries: cache.size, inflight: inflight.size, generations: repoGenerations.size }),
};
