import { create } from "zustand";
import { notify, notifyDesktop, swallow } from "../../plugin-api/host";
import { rundeckApi } from "./api";
import { rundeck } from "./resources";
import { openRundeckExecution, rundeckSettings, type JobRef } from "./state";
import type { RundeckRunNotice } from "./types.generated";

const useWatching = create<{ ids: ReadonlySet<number> }>(() => ({ ids: new Set() }));

const setWatching = (id: number, on: boolean) =>
    useWatching.setState(({ ids }) => {
        const next = new Set(ids);
        if (on) next.add(id);
        else next.delete(id);
        return { ids: next };
    });

/** Whether Sikemux will say when this run ends. */
export const useWatchingRun = (executionId: number): boolean => useWatching((state) => state.ids.has(executionId));

/**
 * Follows a run the person started until it ends, wherever they are in the app, then tells them how it went:
 * a toast inside Sikemux, and a desktop notification when Sikemux is in the background.
 */
export function watchRun(job: JobRef, executionId: number, hasFocus: () => boolean = () => document.hasFocus()): void {
    if (useWatching.getState().ids.has(executionId) || !rundeckSettings.get().notifyWhenDone) return;
    setWatching(executionId, true);
    const finish = (notice: RundeckRunNotice | undefined) => {
        stream.stop();
        setWatching(executionId, false);
        void rundeck.readAgain().catch(swallow("read the runs again"));
        if (!notice || !rundeckSettings.get().notifyWhenDone) return;
        notify(notice.ok ? "success" : "error", notice.title, { action: { label: "Open", run: () => openRundeckExecution(job, executionId) } });
        if (!hasFocus()) void notifyDesktop(notice.title, notice.body).catch(swallow("notify that a run ended"));
    };
    const stream = rundeckApi.watch(
        executionId,
        { jobId: job.jobId, name: job.name, group: job.group, project: job.project },
        {
            onItem: (update) => {
                if (update.terminal) finish(update.notice);
            },
            onEnd: () => setWatching(executionId, false),
            onError: (error) => {
                setWatching(executionId, false);
                swallow("watch the run")(error);
            },
        },
    );
}
