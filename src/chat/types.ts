export interface AcpContentBlock {
    type: string;
    text?: string;
    data?: string;
    mimeType?: string;
    name?: string;
    title?: string;
    uri?: string;
    resource?: unknown;
    [key: string]: unknown;
}

export interface AcpContentChunk {
    content: AcpContentBlock;
    messageId?: string;
}

export interface AcpAvailableCommand {
    name: string;
    description: string;
    input?: { hint?: string };
}

export interface AcpPermissionOption {
    optionId: string;
    name: string;
    kind: "allow_once" | "allow_always" | "reject_once" | "reject_always" | string;
}

export interface AcpPermissionRequest {
    requestId: string;
    sessionId: string;
    toolCall: AcpToolCall;
    options: AcpPermissionOption[];
}

export interface AcpToolCall {
    toolCallId: string;
    title: string;
    kind?: string;
    status?: string;
    content?: unknown[];
    locations?: unknown[];
    rawInput?: unknown;
    rawOutput?: unknown;
    [key: string]: unknown;
}

export type ChatPart =
    | { id: string; kind: "text"; text: string }
    | { id: string; kind: "thought"; text: string }
    | { id: string; kind: "content"; content: AcpContentBlock }
    | { id: string; kind: "tool"; tool: AcpToolCall };

export interface ChatMessage {
    id: string;
    role: "user" | "assistant";
    parts: ChatPart[];
    attachments?: string[];
}

export interface ChatState {
    connection: "connecting" | "installing" | "starting" | "initializing" | "ready" | "stopped" | "error";
    messages: ChatMessage[];
    commands: AcpAvailableCommand[];
    permissions: AcpPermissionRequest[];
    capabilities: Record<string, unknown>;
    setup: Record<string, unknown>;
    plan: unknown;
    usage: unknown;
    running: boolean;
    suppressUserEcho: boolean;
    error: string | null;
    title: string | null;
    stopReason: string | null;
    nextId: number;
    revision: number;
}

export type ChatAction =
    | { type: "reset" }
    | { type: "status"; state: ChatState["connection"] }
    | { type: "ready"; capabilities: Record<string, unknown>; setup: Record<string, unknown> }
    | { type: "local_prompt"; text: string; paths: string[] }
    | { type: "session_update"; update: Record<string, unknown> }
    | { type: "turn_started" }
    | { type: "turn_completed"; stopReason?: string }
    | { type: "permission_requested"; request: AcpPermissionRequest }
    | { type: "permission_cleared"; requestId: string }
    | { type: "error"; message: string };
