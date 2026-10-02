import { invokeCommand as invoke } from "./invoke";
import { getIpcTransport, type IpcUnsubscribe } from "./transport";
import type { AgentPermissionMode, AgentType } from "../state/types";

export interface AcpStartOptions {
    agentId: string;
    provider: AgentType;
    cwd: string;
    resumeId?: string;
    permissionMode: AgentPermissionMode;
    configPath?: string;
    executablePath?: string;
    model?: string;
    effort?: string;
    environmentKeys?: string[];
}

export interface AcpStartResponse {
    sessionId: string;
    capabilities: Record<string, unknown>;
    setup: Record<string, unknown>;
}

/** Whether the background core still runs a chat for the agent. A live one
    replays what it said so far as `acp_event`s before any new event. */
export type AcpAttachment =
    | {
          status: "live";
          start: AcpStartResponse;
          permissionMode: AgentPermissionMode;
          running: boolean;
          /** A turn ran in this session, so the provider keeps it. */
          turned: boolean;
      }
    | { status: "missing" }
    /* It said more than the core keeps, so it starts again from the provider's history. */
    | { status: "restart" };

export interface AcpAttachOptions {
    agentId: string;
    provider: AgentType;
    cwd: string;
    configPath?: string;
}

/** A chat agent the background core runs. */
export interface AcpChat {
    agentId: string;
    provider: string;
    cwd: string;
    sessionId: string | null;
    state: "starting" | "ready" | "stopped";
    running: boolean;
    pendingPermissions: string[];
    /** The paired device that started it; null when this app did. */
    startedBy: string | null;
    /** The app's launcher a device started it with. */
    launcher: string | null;
    permissionMode: string;
    model: string | null;
    effort: string | null;
}

/** Something read elsewhere and handed to the agent whole, such as an issue and its comments. */
export interface PromptContext {
    uri: string;
    title: string;
    text: string;
}

export interface AcpEvent {
    agentId: string;
    /** `reattach`: the core came back with this chat still running, so take it up again. */
    /** `prompt`: what someone on another device sent the agent. */
    kind: "status" | "ready" | "session_update" | "prompt" | "turn_started" | "turn_completed" | "permission_request" | "error" | "reattach";
    /** A `session_update` carries `updates`: a frame's worth of them at once. */
    payload: Record<string, unknown>;
}

export const acpApi = {
    start: (options: AcpStartOptions): Promise<AcpStartResponse> =>
        invoke<AcpStartResponse>("acp_start", {
            agentId: options.agentId,
            provider: options.provider,
            cwd: options.cwd,
            resumeId: options.resumeId ?? null,
            permissionMode: options.permissionMode,
            configPath: options.configPath ?? null,
            executablePath: options.executablePath ?? null,
            model: options.model ?? null,
            effort: options.effort ?? null,
            environmentKeys: options.environmentKeys ?? [],
        }),
    attach: (options: AcpAttachOptions): Promise<AcpAttachment> =>
        invoke<AcpAttachment>("acp_attach", {
            agentId: options.agentId,
            provider: options.provider,
            cwd: options.cwd,
            configPath: options.configPath ?? null,
        }),
    list: (): Promise<AcpChat[]> => invoke<AcpChat[]>("acp_list"),
    setPermissionMode: (agentId: string, permissionMode: AgentPermissionMode): Promise<void> =>
        invoke<void>("acp_set_permission_mode", { agentId, permissionMode }),
    setConfig: (agentId: string, configId: string, value: string): Promise<Record<string, unknown>> =>
        invoke("acp_set_config", { agentId, configId, value }),
    prompt: (agentId: string, text: string, paths: string[], context: PromptContext[] = []): Promise<void> =>
        invoke<void>("acp_prompt", { agentId, text, paths, context }),
    steer: (agentId: string, text: string, paths: string[], context: PromptContext[] = []): Promise<string> =>
        invoke<string>("acp_steer", { agentId, text, paths, context }),
    cancel: (agentId: string): Promise<void> => invoke<void>("acp_cancel", { agentId }),
    stopTask: (agentId: string, taskId: string): Promise<void> => invoke<void>("acp_stop_task", { agentId, taskId }),
    permissionReply: (agentId: string, requestId: string, optionId?: string): Promise<void> =>
        invoke<void>("acp_permission_reply", { agentId, requestId, optionId: optionId ?? null }),
    stop: (agentId: string): Promise<void> => invoke<void>("acp_stop", { agentId }),
    subscribe: (listener: (event: AcpEvent) => void, signal?: AbortSignal): Promise<IpcUnsubscribe> =>
        getIpcTransport().subscribe<AcpEvent>("acp_event", (event) => listener(event.payload), { signal }),
};
