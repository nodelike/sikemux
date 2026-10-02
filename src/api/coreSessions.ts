import { invokeCommand as invoke } from "./invoke";
import type { TaskSource } from "../tasks/taskRegistry";

/** A task's launch request without its environment. */
export interface CoreTaskInfo {
    readonly executionId: string;
    readonly terminalKey: string;
    readonly taskId: string;
    readonly label: string;
    readonly project: string;
    readonly source: TaskSource;
    readonly command: string;
    readonly cwd: string;
    readonly agentId: string | null;
}

export interface CoreSessionExit {
    readonly code: number | null;
    readonly signal: string | null;
}

/** A terminal or task the background core holds, running or kept after it ended. */
export interface CoreSession {
    readonly id: number;
    readonly kind: "terminal" | "task";
    readonly pid: number | null;
    readonly running: boolean;
    readonly project: string | null;
    readonly paneId: string | null;
    readonly agentId: string | null;
    readonly agentType: string | null;
    readonly task: CoreTaskInfo | null;
    readonly exit: CoreSessionExit | null;
    /** Sikemux asked for its process to end. */
    readonly killed: boolean;
    /** The paired device that started it; null when this app did. */
    readonly startedBy: string | null;
}

export const coreSessionsApi = {
    list: () => invoke<CoreSession[]>("pty_sessions"),
    kill: (id: number) => invoke<void>("pty_kill", { id }),
    quitAndStopEverything: () => invoke<void>("app_quit_and_stop_everything"),
};
