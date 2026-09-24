import { createPluginBackend, isPluginFailure } from "../../plugin-api/backend";
import { invalidate } from "../../plugin-api/resources";
import { RUNDECK_PLUGIN_ID } from "./kinds";

const backend = createPluginBackend(RUNDECK_PLUGIN_ID);

export function isAuthFailure(error: unknown): boolean {
    if (!isPluginFailure(error)) return false;
    return error.category === "auth" || error.category === "unconfigured" || (error.category === "http" && error.status === 401);
}

const FORBIDDEN_ACTION: Record<string, string> = {
    projects: "list projects",
    jobs: "list this project's jobs",
    jobIndex: "list jobs",
    jobDetail: "read this job",
    branchesMatrix: "read this project's executions",
    jobCells: "read these jobs' executions",
    executions: "read this job's executions",
    execution: "read this execution",
    executionState: "read this execution",
    run: "run this job",
    abort: "abort this execution",
    plan: "read this job's executions",
};

/** A lapsed sign-in makes every cached answer stale, so the next read asks again and lands on the login form. */
async function rndInvoke<T>(method: string, params?: unknown): Promise<T> {
    try {
        return await backend.call<T>(method, params);
    } catch (error) {
        if (isAuthFailure(error)) invalidate((kind) => kind.startsWith("rnd."));
        if (isPluginFailure(error, "forbidden")) {
            const action = FORBIDDEN_ACTION[method] ?? "do that";
            throw { ...error, message: `You don't have permission to ${action}. ${error.message}`.trim() };
        }
        throw error;
    }
}

export function errorMessage(error: unknown): string {
    if (typeof error === "object" && error && "message" in error) return String((error as { message: unknown }).message);
    return String(error);
}

export interface RundeckStatus {
    configured: boolean;
    url: string;
    user: string;
    token_present: boolean;
    rundeck_version: string | null;
    ok: boolean;
    auth_failed: boolean;
    message: string | null;
    allow_insecure_private_http: boolean;
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

export interface RundeckProject {
    name: string;
    description: string | null;
}

export interface RundeckJob {
    id: string;
    name: string;
    group: string | null;
    project: string;
    description: string | null;
    href: string | null;
    permalink: string | null;
    enabled: boolean | null;
    scheduled: boolean | null;
    scheduleEnabled: boolean | null;
}

export interface JobIndexEntry {
    project: string;
    jobs: RundeckJob[];
    error: string | null;
}

export interface JobOption {
    name: string;
    label: string | null;
    description: string | null;
    required: boolean;
    secure: boolean;
    value_exposed: boolean;
    default: string | null;
    values: string[] | null;
    values_url: string | null;
    enforced: boolean;
    multivalued: boolean;
    delimiter: string | null;
    is_date: boolean;
    date_format: string | null;
    kind: "text" | "file";
}

export interface JobDetail {
    id: string;
    name: string;
    group: string | null;
    project: string;
    description: string | null;
    execution_enabled: boolean;
    schedule_enabled: boolean;
    scheduled: boolean;
    node_filter: string | null;
    options: JobOption[];
    steps: string[];
}

export interface OptionValue {
    name: string;
    value: string;
}

export interface ExecSummary {
    execution_id: number;
    status: string;
    custom_status: string | null;
    user: string;
    started_at: string | null;
    ended_at: string | null;
    permalink: string | null;
    branch: string | null;
    options: Record<string, string>;
}

export interface MatrixCell {
    service: string;
    name: string;
    job_id: string;
    group: string | null;
    enabled: boolean | null;
    scheduled: boolean | null;
    latest: ExecSummary | null;
    deployed: ExecSummary | null;
    error: string | null;
}

export interface MatrixResult {
    project: string;
    cells: MatrixCell[];
    error: string | null;
    partial: boolean;
    elapsed_ms: number;
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
}

export interface LogEntry {
    time: string | null;
    level: string | null;
    log: string | null;
    user: string | null;
    stepctx: string | null;
    node: string | null;
}

export interface LogTick {
    entries: LogEntry[];
    offset: string;
    completed: boolean;
    failed: boolean;
    error: string | null;
}

export type BranchRelation =
    | "same"
    | "target-contains-deployed"
    | "target-missing-deployed"
    | "unknown-no-deployed-branch"
    | "unknown-deployed-not-on-origin"
    | "unknown-target-not-on-origin"
    | "unknown-no-repo";

export type PushAction = "will-push-current" | "will-not-push-different-branch" | "will-not-push-no-repo" | "will-not-push-detached";

export interface PlanRequest {
    jobId: string;
    project: string;
    service: string;
    targetBranch: string;
    repoPath: string;
    branchOptions: string[];
}

export interface PlanResult {
    project: string;
    service: string;
    target_branch: string;
    deployed_branch: string | null;
    branch_relation: BranchRelation;
    branch_relation_detail: string | null;
    git_root: string | null;
    current_branch: string | null;
    head_sha: string | null;
    dirty: boolean;
    upstream: string | null;
    ahead: number | null;
    behind: number | null;
    remote_target_exists: boolean;
    push_action: PushAction;
}

export const rundeckApi = {
    status: () => backend.call<RundeckStatus>("status"),
    login: (req: RundeckLoginRequest) => rndInvoke<RundeckLoginResult>("login", req),
    loginWithToken: (req: RundeckTokenLoginRequest) => rndInvoke<RundeckLoginResult>("loginWithToken", req),
    logout: () => backend.call<null>("logout"),

    projects: () => rndInvoke<RundeckProject[]>("projects"),
    jobs: (project: string) => rndInvoke<RundeckJob[]>("jobs", { project }),
    jobIndex: () => rndInvoke<JobIndexEntry[]>("jobIndex"),
    jobDetail: (jobId: string) => rndInvoke<JobDetail>("jobDetail", { jobId }),
    optionValues: (url: string) => rndInvoke<OptionValue[]>("optionValues", { url }),
    branchesMatrix: (project: string, branchOptions: string[]) => rndInvoke<MatrixResult>("branchesMatrix", { project, branchOptions }),
    jobCells: (jobs: RundeckJob[], branchOptions: string[]) => rndInvoke<MatrixCell[]>("jobCells", { jobs, branchOptions }),

    executions: (jobId: string, project: string, max = 25, onlySucceeded = false) =>
        rndInvoke<RundeckExecution[]>("executions", { jobId, project, max, onlySucceeded }),
    execution: (executionId: number) => rndInvoke<RundeckExecution>("execution", { executionId }),
    executionState: (executionId: number) => rndInvoke<RundeckWorkflowState>("executionState", { executionId }),
    run: (req: RunRequest) => rndInvoke<RunResult>("run", req),
    abort: (executionId: number) => rndInvoke<AbortResult>("abort", { executionId }),

    watchStart: (executionId: number, onUpdate: (u: WatchUpdate) => void) => backend.openStream("watch", { executionId }, onUpdate),
    watchStop: (id: number) => backend.closeStream(id),

    logsStart: (executionId: number, offset: string | null, backlog: number | null, onChunk: (c: LogTick) => void) =>
        backend.openStream("logs", { executionId, offset, backlog }, onChunk),
    logsStop: (id: number) => backend.closeStream(id),

    plan: (req: PlanRequest) => rndInvoke<PlanResult>("plan", req),
};
