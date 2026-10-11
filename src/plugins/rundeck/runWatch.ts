import { create } from "zustand";
import { notify, notifyDesktop, swallow } from "../../plugin-api/host";
import { invalidate } from "../../plugin-api/resources";
import { rundeckApi, type RundeckExecution, type WatchUpdate } from "./api";
import { lostRunNotice, runNotice, type RunNotice } from "./runNotice";
import { openRundeckExecution, rundeckSettings, type JobRef } from "./state";

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
export async function watchRun(job: JobRef, executionId: number, hasFocus: () => boolean = () => document.hasFocus()): Promise<void> {
    if (useWatching.getState().ids.has(executionId) || !rundeckSettings.get().notifyWhenDone) return;
    setWatching(executionId, true);
    let streamId: number | null = null;
    let ended = false;
    const stop = () => {
        setWatching(executionId, false);
        if (streamId !== null) void rundeckApi.watchStop(streamId).catch(swallow("stop watching a run"));
    };
    const finish = (update: WatchUpdate) => {
        ended = true;
        stop();
        invalidate((kind, args) => (kind === "rnd.executions" && args[0] === job.jobId) || (kind === "rnd.matrix" && args[0] === job.project));
        if (!rundeckSettings.get().notifyWhenDone) return;
        const notice = endedNotice(job, executionId, update);
        notify(notice.ok ? "success" : "error", notice.title, { action: { label: "Open", run: () => openRundeckExecution(job, executionId) } });
        if (!hasFocus()) void notifyDesktop(notice.title, notice.body).catch(swallow("notify that a run ended"));
    };
    try {
        streamId = await rundeckApi.watchStart(executionId, (update) => {
            if (!ended && update.terminal) finish(update);
        });
        if (ended) stop();
    } catch (error) {
        setWatching(executionId, false);
        throw error;
    }
}

/** The last update can lack the run itself when Rundeck stopped answering; its step state may still say how it ended. */
function endedNotice(job: JobRef, executionId: number, update: WatchUpdate): RunNotice {
    const { branchOptions } = rundeckSettings.get();
    if (update.execution) return runNotice(update.execution, branchOptions);
    const status = update.state?.executionState;
    if (!status) return lostRunNotice(job.name, job.group, executionId);
    const known: RundeckExecution = {
        id: executionId,
        status,
        customStatus: null,
        user: null,
        project: job.project,
        "date-started": null,
        "date-ended": null,
        permalink: null,
        job: { id: job.jobId, name: job.name, group: job.group, project: job.project, options: null },
        argstring: null,
    };
    return runNotice(known, branchOptions);
}
