import type { TaskExecutionBackend, TaskExecutionRequest, TaskTerminalSurface } from "../tasks/runtime";
import type { HarnessEvents } from "./events";

export interface HarnessRun {
    executionId: string;
    taskId: string;
    project: string;
    status: "starting" | "running" | "completed" | "failed" | "stopping" | "stopped";
    ptyId?: number;
    exitCode?: number;
    signal?: string | null;
    error?: string;
    previewUrl?: string;
}

interface Entry {
    run: HarnessRun;
    started: Promise<HarnessRun>;
    abort: AbortController;
    stop?: Promise<HarnessRun>;
}

export class HarnessTasks {
    private readonly entries = new Map<string, Entry>();
    private readonly keys = new Map<string, { taskId: string; executionId: string }>();

    constructor(
        private readonly backend: TaskExecutionBackend,
        private readonly surface: TaskTerminalSurface,
        private readonly events: HarnessEvents,
    ) {}

    list(project: string): HarnessRun[] {
        return [...this.entries.values()].filter((entry) => entry.run.project === project).map((entry) => ({ ...entry.run }));
    }

    get(project: string, executionId: string): HarnessRun {
        return { ...this.entry(project, executionId).run };
    }

    existing(project: string, taskId: string, key: string): Promise<HarnessRun> | undefined {
        const previous = this.keys.get(JSON.stringify([project, key]));
        if (previous) {
            if (previous.taskId !== taskId) throw new Error("idempotencyKey was already used for another task");
            const entry = this.entry(project, previous.executionId);
            return entry.started.then(() => ({ ...entry.run }));
        }
        return undefined;
    }

    start(request: Omit<TaskExecutionRequest, "executionId" | "terminalKey">, key: string, previewUrl?: string): Promise<HarnessRun> {
        const previous = this.existing(request.project, request.taskId, key);
        if (previous) return previous;
        const active = [...this.entries.values()].find(
            ({ run }) => run.project === request.project && run.taskId === request.taskId && ["starting", "running", "stopping"].includes(run.status),
        );
        if (this.keys.size >= 256) throw new Error("Harness idempotency capacity reached; restart Sikemux to clear run history");
        if (active) {
            this.keys.set(JSON.stringify([request.project, key]), { taskId: request.taskId, executionId: active.run.executionId });
            return active.started.then(() => ({ ...active.run }));
        }
        if (this.entries.size >= 128) throw new Error("Harness run capacity reached; restart Sikemux to clear run history");
        const executionId = crypto.randomUUID();
        const run: HarnessRun = { executionId, taskId: request.taskId, project: request.project, status: "starting", previewUrl };
        const abort = new AbortController();
        const entry: Entry = { run, abort, started: Promise.resolve(run) };
        this.entries.set(executionId, entry);
        this.keys.set(JSON.stringify([request.project, key]), { taskId: request.taskId, executionId });
        this.publish(run);
        entry.started = this.launch(entry, { ...request, executionId, terminalKey: JSON.stringify(["harness", request.project, request.taskId]) });
        void entry.started.catch(() => {});
        return entry.started;
    }

    private async launch(entry: Entry, request: TaskExecutionRequest): Promise<HarnessRun> {
        const { run } = entry;
        try {
            const started = await this.backend.start(request);
            run.ptyId = started.ptyId;
            run.status = "running";
            this.publish(run);
            void Promise.resolve(started.completion).then(
                (exit) => {
                    run.exitCode = exit.code;
                    run.signal = exit.signal;
                    if (run.status !== "stopped") run.status = run.status === "stopping" ? "stopped" : exit.code === 0 ? "completed" : "failed";
                    this.publish(run);
                },
                () => {
                    run.status = "failed";
                    run.error = "Task completion could not be observed";
                    this.publish(run);
                },
            );
            await this.surface.open({ ...request, ptyId: started.ptyId, signal: entry.abort.signal });
            return { ...run };
        } catch (error) {
            if (run.ptyId !== undefined)
                await Promise.resolve(this.backend.stop(run.ptyId)).catch(() => {
                    run.error = "Task launch failed and process cleanup failed";
                });
            run.status = "failed";
            run.error ??= "Task could not be started or its terminal could not be opened";
            this.publish(run);
            throw error;
        }
    }

    stop(project: string, executionId: string): Promise<HarnessRun> {
        const entry = this.entry(project, executionId);
        if (entry.stop) return entry.stop;
        const operation = async () => {
            await entry.started.catch(() => {});
            const { run } = entry;
            if (run.ptyId === undefined || ["completed", "stopped"].includes(run.status) || (run.status === "failed" && run.exitCode !== undefined))
                return { ...run };
            run.status = "stopping";
            this.publish(run);
            try {
                await this.backend.stop(run.ptyId);
                run.status = "stopped";
                this.publish(run);
                return { ...run };
            } catch (error) {
                run.status = "failed";
                run.error = "Task could not be stopped";
                this.publish(run);
                throw error;
            }
        };
        entry.stop = operation().finally(() => {
            entry.stop = undefined;
        });
        return entry.stop;
    }

    output(ptyId: number): void {
        const entry = [...this.entries.values()].find(({ run }) => run.ptyId === ptyId);
        if (entry) this.events.publish({ project: entry.run.project, kind: "task.output", executionId: entry.run.executionId });
    }

    closeProject(project: string): void {
        for (const run of this.list(project)) void this.stop(project, run.executionId).catch(() => {});
    }

    private entry(project: string, executionId: string): Entry {
        const entry = this.entries.get(executionId);
        if (!entry || entry.run.project !== project) throw new Error("Task execution does not belong to this project");
        return entry;
    }

    private publish(run: HarnessRun): void {
        this.events.publish({ project: run.project, kind: `task.${run.status}`, executionId: run.executionId });
    }
}
