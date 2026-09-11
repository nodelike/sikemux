import { loadProjectFilesSnapshot } from "../projects/application";

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

function nextInvalidationGeneration(): number {
    if (invalidationSequence >= Number.MAX_SAFE_INTEGER) throw new RangeError("file cache generation space exhausted");
    invalidationSequence += 1;
    return invalidationSequence;
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

function assertSnapshot(value: unknown): ProjectFilesSnapshot {
    if (typeof value !== "object" || value === null || Array.isArray(value)) {
        throw new TypeError("native project file snapshot is malformed");
    }
    const scanIdDescriptor = Object.getOwnPropertyDescriptor(value, "scanId");
    const filesDescriptor = Object.getOwnPropertyDescriptor(value, "files");
    const scanId = scanIdDescriptor && "value" in scanIdDescriptor ? scanIdDescriptor.value : undefined;
    const files = filesDescriptor && "value" in filesDescriptor ? filesDescriptor.value : undefined;
    if (!Number.isSafeInteger(scanId) || (scanId as number) <= 0 || !Array.isArray(files) || !files.every((path) => typeof path === "string")) {
        throw new TypeError("native project file snapshot is malformed");
    }
    return { scanId: scanId as number, files };
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

    const promise = loadProjectFilesSnapshot(repo)
        .then(assertSnapshot)
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
        const generation = nextInvalidationGeneration();
        if (repo) repoGenerations.set(repo, generation);
        else globalGeneration = generation;
    },
    evict: (repo: string) => {
        cache.delete(repo);
        if (!inflight.has(repo)) repoGenerations.delete(repo);
    },
    stats: () => ({ cacheEntries: cache.size, inflight: inflight.size, generations: repoGenerations.size }),
};
