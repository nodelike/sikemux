import { coreCall, coreRequest, onHost, type CoreRequest } from "./core";
import { getState } from "../state/store";
import type { ChatInfo } from "./generated/wire";
import { invokeCommand as invoke } from "./invoke";
import { getIpcTransport, type IpcUnsubscribe } from "./transport";
import type { AgentPermissionMode, AgentType } from "../state/types";

/** One of the person's accounts with the chat's provider: a profile, by its directory. */
export interface AcpAccount {
    id: string;
    label: string;
    configPath?: string;
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
          /** The core's run of this chat, which paging back names. */
          feed: string;
          /** Set when the replay holds only the chat's last turns: `history` pages back from it. */
          olderBefore: number | null;
      }
    | { status: "missing" };

/** Turns from before what the page shows, and where the page before them starts. */
export interface AcpHistory {
    events: { kind: string; payload: Record<string, unknown> }[];
    olderBefore: number | null;
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
    kind:
        | "status"
        | "ready"
        | "session_update"
        | "prompt"
        | "turn_started"
        | "turn_completed"
        | "permission_request"
        | "permission_cleared"
        | "error"
        | "reattach";
    /** A `session_update` carries `updates`: a frame's worth of them at once. */
    payload: Record<string, unknown>;
}

/** The host a chat runs on, which every request about it goes to. */
function hostOf(agentId: string): string | undefined {
    return getState().agents[agentId]?.host;
}

function ask<Reply>(request: CoreRequest & { agentId: string }): Promise<Reply> {
    return coreRequest<Reply>(request, hostOf(request.agentId));
}

function done(request: CoreRequest & { agentId: string }): Promise<void> {
    return ask<unknown>(request).then(() => undefined);
}

function call<T>(method: string, params: { agentId: string } & Record<string, unknown>): Promise<T> {
    return coreCall<T>(method, params, hostOf(params.agentId));
}

export const acpApi = {
    /** Starts the agent's chat as the core keeps it, unless it runs already; answers once its session is ready. */
    startChat: (agentId: string): Promise<void> =>
        hostOf(agentId) ? Promise.reject(new Error("This chat has ended on its host.")) : coreCall<void>("startChat", { agentId }),
    attach: (agentId: string): Promise<AcpAttachment> => invoke<AcpAttachment>("acp_attach", onHost({ agentId }, hostOf(agentId))),
    history: (agentId: string, feed: string, before: number, turns: number): Promise<AcpHistory> =>
        ask<AcpHistory>({ op: "acpHistory", agentId, feed, before, turns }),
    list: () => coreRequest<{ chats: ChatInfo[] }>({ op: "acpList" }).then(({ chats }) => chats),
    setPermissionMode: (agentId: string, mode: AgentPermissionMode): Promise<void> => done({ op: "acpSetPermissionMode", agentId, mode }),
    setConfig: (agentId: string, configId: string, value: string): Promise<Record<string, unknown>> =>
        ask<{ value: Record<string, unknown> }>({ op: "acpSetConfig", agentId, configId, value }).then((reply) => reply.value),
    prompt: (agentId: string, text: string, paths: string[], context: PromptContext[] = [], messageId?: string): Promise<void> =>
        done({ op: "acpPrompt", agentId, messageId, text, paths, context }),
    edit: (agentId: string, messageId: string, text: string, paths: string[], context: PromptContext[]): Promise<void> =>
        done({ op: "acpEdit", agentId, messageId, text, paths, context }),
    steer: (agentId: string, text: string, paths: string[], context: PromptContext[] = []): Promise<string> =>
        ask<{ outcome: string }>({ op: "acpSteer", agentId, text, paths, context }).then(({ outcome }) => outcome),
    /** Puts the waiting messages `ids`, then `message` when given, into the running turn as one; a turn that just ended starts again with them. */
    steerQueued: (agentId: string, ids: string[], message?: { text: string; paths: string[]; context: PromptContext[] }): Promise<void> =>
        call<void>("steerQueued", { agentId, ids, message: message ?? null }),
    /** Takes back waiting messages before they go out. */
    dropQueued: (agentId: string, ids: string[]): Promise<void> => call<void>("dropQueued", { agentId, ids }),
    /** Stops the running turn; the messages waiting behind it are taken back. */
    cancel: (agentId: string): Promise<void> => done({ op: "acpCancel", agentId }),
    stopTask: (agentId: string, taskId: string): Promise<void> => done({ op: "acpStopTask", agentId, taskId }),
    permissionReply: (agentId: string, requestId: string, optionId?: string): Promise<void> =>
        done({ op: "acpPermissionReply", agentId, requestId, optionId: optionId ?? null }),
    stop: (agentId: string): Promise<void> => invoke<void>("acp_stop", onHost({ agentId }, hostOf(agentId))),
    /** Stops the chat unless its agent is still at work, which only the person stops. Answers whether it stopped. */
    stopIfIdle: (agentId: string): Promise<boolean> =>
        hostOf(agentId) ? Promise.resolve(false) : coreCall<{ stopped: boolean }>("stopChatIfIdle", { agentId }).then(({ stopped }) => stopped),
    switchAccount: (agentId: string, provider: AgentType, account: AcpAccount): Promise<void> =>
        invoke<void>("acp_switch_account", onHost({ agentId, provider, account }, hostOf(agentId))),
    subscribe: (listener: (event: AcpEvent) => void, signal?: AbortSignal): Promise<IpcUnsubscribe> =>
        getIpcTransport().subscribe<AcpEvent>("acp_event", (event) => listener(event.payload), { signal }),
};
