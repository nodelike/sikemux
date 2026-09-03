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
    environmentKeys?: string[];
}

export interface AcpStartResponse {
    sessionId: string;
    capabilities: Record<string, unknown>;
    setup: Record<string, unknown>;
}

export interface AcpEvent {
    agentId: string;
    kind: "status" | "ready" | "session_update" | "turn_started" | "turn_completed" | "permission_request" | "error";
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
            environmentKeys: options.environmentKeys ?? [],
        }),
    prompt: (agentId: string, text: string, paths: string[]): Promise<void> => invoke<void>("acp_prompt", { agentId, text, paths }),
    cancel: (agentId: string): Promise<void> => invoke<void>("acp_cancel", { agentId }),
    permissionReply: (agentId: string, requestId: string, optionId?: string): Promise<void> =>
        invoke<void>("acp_permission_reply", { agentId, requestId, optionId: optionId ?? null }),
    stop: (agentId: string): Promise<void> => invoke<void>("acp_stop", { agentId }),
    subscribe: (listener: (event: AcpEvent) => void, signal?: AbortSignal): Promise<IpcUnsubscribe> =>
        getIpcTransport().subscribe<AcpEvent>("acp_event", (event) => listener(event.payload), { signal }),
};
