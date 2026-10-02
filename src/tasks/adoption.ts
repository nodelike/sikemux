import type { CoreSession, CoreTaskInfo } from "../api/coreSessions";
import type { TaskExecutionStart, TaskTerminalOpenRequest } from "./runtime";
import type { ResolvedTaskDefinition } from "./taskRegistry";

/** Where a task the core kept goes back to: the command deck, or the harness that an agent started it through. */
export type TaskOrigin = { readonly kind: "deck" | "harness"; readonly project: string; readonly taskId: string };

export function taskOrigin(task: CoreTaskInfo): TaskOrigin | null {
    let key: unknown;
    try {
        key = JSON.parse(task.terminalKey);
    } catch {
        return null;
    }
    if (!Array.isArray(key) || key.length !== 3 || key[1] !== task.project || key[2] !== task.taskId) return null;
    if (key[0] === "task") return { kind: "deck", project: task.project, taskId: task.taskId };
    if (key[0] === "harness") return { kind: "harness", project: task.project, taskId: task.taskId };
    return null;
}

export interface TaskAdoptionTargets {
    watch(ptyId: number): Promise<TaskExecutionStart>;
    adoptDeckTask(task: ResolvedTaskDefinition, executionId: string, started: TaskExecutionStart): Promise<void>;
    /** The core keeps harness runs itself; the page only shows the terminal again. */
    showHarnessTerminal(request: Omit<TaskTerminalOpenRequest, "signal" | "background">): void;
}

/* Only running tasks come back: the deck shows one running task per project
   and nothing of finished ones, and a finished harness run needs no terminal
   for an agent to read how it ended. */
export async function adoptCoreTasks(sessions: readonly CoreSession[], targets: TaskAdoptionTargets): Promise<number> {
    let running = 0;
    for (const session of sessions) {
        const task = session.kind === "task" ? session.task : null;
        const origin = task && taskOrigin(task);
        if (!task || !origin || !session.running) continue;
        if (origin.kind === "harness") {
            targets.showHarnessTerminal({
                executionId: task.executionId,
                terminalKey: task.terminalKey,
                ptyId: session.id,
                taskId: task.taskId,
                label: task.label,
                project: task.project,
                source: task.source,
                cwd: task.cwd,
                agentId: task.agentId ?? undefined,
            });
            running += 1;
            continue;
        }
        let started: TaskExecutionStart;
        try {
            started = await targets.watch(session.id);
        } catch {
            continue;
        }
        const definition = {
            id: task.taskId,
            taskId: task.taskId,
            label: task.label,
            project: task.project,
            source: task.source,
            command: task.command,
            cwd: task.cwd,
            env: {},
        };
        void targets.adoptDeckTask(definition, task.executionId, started).catch(() => {});
        running += 1;
    }
    return running;
}
