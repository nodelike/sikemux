import { useMemo, type ReactNode } from "react";
import { WorkerPoolContextProvider, type WorkerInitializationRenderOptions, type WorkerPoolOptions } from "@pierre/diffs/react";
import { DIFF_WORD_MAX_LENGTH } from "./DiffEditor";

function workerCount(): number {
    const available = typeof navigator === "undefined" ? 2 : (navigator.hardwareConcurrency ?? 2);
    return Math.max(1, Math.min(4, available - 1));
}

export function DiffWorkerProvider({ children }: { children: ReactNode }) {
    const poolOptions = useMemo<WorkerPoolOptions>(
        () => ({
            workerFactory: () => new Worker(new URL("@pierre/diffs/worker/worker.js", import.meta.url), { type: "module", name: "sikemux-diff" }),
            poolSize: workerCount(),
            totalASTLRUCacheSize: 192,
        }),
        [],
    );
    const highlighterOptions = useMemo<WorkerInitializationRenderOptions>(
        () => ({
            langs: ["text"],
            lineDiffType: "none",
            maxLineDiffLength: DIFF_WORD_MAX_LENGTH,
        }),
        [],
    );

    return (
        <WorkerPoolContextProvider poolOptions={poolOptions} highlighterOptions={highlighterOptions}>
            {children}
        </WorkerPoolContextProvider>
    );
}
