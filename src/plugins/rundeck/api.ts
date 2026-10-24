import { createPluginBackend, isPluginFailure, type PluginStreamHandlers } from "../../plugin-api/backend";
import { RUNDECK_PLUGIN } from "./kinds";
import type { RundeckJobOption, RundeckRunCheck, RundeckRunNotice, RundeckStyledText } from "./types.generated";

const backend = createPluginBackend(RUNDECK_PLUGIN.id);

const FORBIDDEN_ACTION: Record<string, string> = {
    run: "run this job",
    abort: "abort this execution",
    checkRun: "read this job's executions",
};

async function rndInvoke<T>(method: string, params?: unknown): Promise<T> {
    try {
        return await backend.call<T>(method, params);
    } catch (error) {
        if (isPluginFailure(error, "forbidden")) {
            const action = FORBIDDEN_ACTION[method] ?? "do that";
            throw { ...error, message: `You don't have permission to ${action}. ${error.message}`.trim() };
        }
        throw error;
    }
}

export interface RundeckStatus {
    configured: boolean;
    url: string;
    user: string;
    tokenPresent: boolean;
    rundeckVersion: string | null;
    ok: boolean;
    authFailed: boolean;
    message: string | null;
    allowInsecurePrivateHttp: boolean;
}

export interface RundeckLoginRequest {
    url: string;
    user: string;
    password: string;
    allow_insecure_private_http: boolean;
}

export interface RundeckTokenLoginRequest {
    url: string;
    token: string;
    allow_insecure_private_http: boolean;
}

export interface RundeckLoginResult {
    url: string;
    user: string;
    token_set: boolean;
    rundeck_version: string | null;
}

export interface RundeckExecution {
    id: number;
    status: string | null;
    customStatus: string | null;
    user: string | null;
    project: string | null;
    ["date-started"]: { date: string | null; unixtime: number | null } | null;
    ["date-ended"]: { date: string | null; unixtime: number | null } | null;
    permalink: string | null;
    job: {
        id: string | null;
        name: string | null;
        group: string | null;
        project: string | null;
        options: Record<string, string> | null;
    } | null;
    argstring: string | null;
    workflowState?: RundeckWorkflowState | null;
}

export type LogLevel = "DEBUG" | "VERBOSE" | "INFO" | "WARN" | "ERROR";

export interface RunRequest {
    jobId: string;
    options: Record<string, string>;
    loglevel: LogLevel | null;
    filter: string | null;
    runAtTime: string | null;
    asUser: string | null;
}

export interface RunResult {
    id: number;
    permalink: string | null;
    status: string | null;
    recovered: boolean;
}

export interface AbortResult {
    abort: { status: string | null; reason: string | null } | null;
    execution: { id: string; status: string | null } | null;
}

export interface RundeckStep {
    id: string | null;
    stepctx: string | null;
    executionState: string | null;
    startTime: string | null;
    endTime: string | null;
    nodeStep: boolean | null;
}

export interface RundeckWorkflowState {
    executionState: string | null;
    steps: RundeckStep[];
    stepCount: number | null;
    completed: boolean | null;
}

export interface WatchUpdate {
    execution: RundeckExecution | null;
    state: RundeckWorkflowState | null;
    error: string | null;
    terminal: boolean;
    /** What to tell the person, once the run has ended. */
    notice?: RundeckRunNotice;
}

/** The job a watched run belongs to, so the plugin can name it when Rundeck stops saying. */
export interface WatchedJob {
    jobId: string;
    name: string;
    group: string | null;
    project: string;
}

export interface LogEntry {
    time: string | null;
    level: string | null;
    log: string | null;
    user: string | null;
    stepctx: string | null;
    node: string | null;
    /** The line in its styles, when its terminal codes set any. */
    runs?: RundeckStyledText[];
}

export interface LogTick {
    entries: LogEntry[];
    offset: string;
    completed: boolean;
    failed: boolean;
    error: string | null;
}

export interface RunCheckRequest {
    options: RundeckJobOption[];
    values: Record<string, string>;
    remote: Record<string, string[] | null>;
    /** Given to also ask whether a run of the job goes on. */
    jobId?: string;
    project?: string;
}

/** The lists people edit in the plugin's settings, kept by the core. */
export interface RundeckLists {
    branchOptions: string[];
    prodEnvs: string[];
}

export const rundeckApi = {
    status: () => backend.call<RundeckStatus>("status"),
    login: (req: RundeckLoginRequest) => rndInvoke<RundeckLoginResult>("login", req),
    loginWithToken: (req: RundeckTokenLoginRequest) => rndInvoke<RundeckLoginResult>("loginWithToken", req),
    logout: () => backend.call<null>("logout"),

    lists: () => backend.call<RundeckLists>("lists"),
    setLists: (lists: Partial<RundeckLists>) => backend.call<null>("setLists", lists),

    run: (req: RunRequest) => rndInvoke<RunResult>("run", req),
    abort: (executionId: number) => rndInvoke<AbortResult>("abort", { executionId }),

    watch: (executionId: number, job: WatchedJob | null, handlers: PluginStreamHandlers<WatchUpdate>) =>
        backend.stream("watch", { executionId, job }, handlers),
    logs: (executionId: number, offset: string | null, handlers: PluginStreamHandlers<LogTick>) =>
        backend.stream("logs", { executionId, offset, backlog: null }, handlers),

    checkRun: (req: RunCheckRequest) => rndInvoke<RundeckRunCheck>("checkRun", req),
};
