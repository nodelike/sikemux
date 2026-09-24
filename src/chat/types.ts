import type { ToolDiff } from "./diff";

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

export interface AcpAsyncTask {
    asyncTaskId: string;
    name: string;
    taskType: string;
    description: string;
    state: "running" | "paused" | "completed" | "failed" | "stopped";
    canStop: boolean;
    summary?: string;
    lastToolName?: string;
    outputFilePath?: string;
    usage?: { totalTokens: number; toolUses: number; durationMs: number };
}

/* A subagent runs as its own ACP session, so its transcript is kept whole
   rather than spliced into the parent's. */
export interface AcpSubagent {
    sessionId: string;
    name: string;
    task: string;
    state: "running" | "completed" | "failed" | "cancelled" | "disconnected";
    messages: ChatMessage[];
    nextId: number;
}

/* What a background task left behind once it ended, kept in the transcript
   because the live task above the composer goes away with it. */
export interface AcpTaskNotice {
    name: string;
    state: "completed" | "failed" | "stopped";
    summary?: string;
}

export type ChatPart =
    | { id: string; kind: "text"; text: string }
    | { id: string; kind: "thought"; text: string }
    | { id: string; kind: "content"; content: AcpContentBlock }
    /* A finished call is read once and kept as what the transcript shows: the
       change it made, and what it left behind when it failed. */
    | { id: string; kind: "tool"; tool: AcpToolCall; diff?: ToolDiff; failure?: string; startedAt?: number; endedAt?: number }
    | { id: string; kind: "subagent"; subagent: AcpSubagent }
    | { id: string; kind: "notice"; notice: AcpTaskNotice };

export interface ChatMessage {
    id: string;
    role: "user" | "assistant";
    parts: ChatPart[];
    attachments?: string[];
    /* When the first and last characters of a streamed answer landed, and how
       many arrived between them — what the transcript's speed reading is
       worked out from. A replayed message has none of these. */
    streamStartedAt?: number;
    streamEndedAt?: number;
    streamChars?: number;
}

export interface ChatState {
    connection: "connecting" | "installing" | "starting" | "initializing" | "ready" | "stopped" | "error";
    messages: ChatMessage[];
    commands: AcpAvailableCommand[];
    permissions: AcpPermissionRequest[];
    tasks: AcpAsyncTask[];
    capabilities: Record<string, unknown>;
    setup: Record<string, unknown>;
    plan: unknown;
    usage: ContextUsage | null;
    running: boolean;
    /* The agent started this turn on its own, woken by a message from another
       session or a finished background task, so no prompt of ours will end it. */
    unprompted: boolean;
    suppressUserEcho: boolean;
    error: string | null;
    title: string | null;
    stopReason: string | null;
    nextId: number;
    revision: number;
    awaitingReplay: boolean;
}

export type ChatAction =
    | { type: "reset"; hold?: boolean }
    | { type: "config"; options: unknown }
    | { type: "status"; state: ChatState["connection"] }
    | { type: "ready"; capabilities: Record<string, unknown>; setup: Record<string, unknown> }
    | { type: "local_prompt"; text: string; paths: string[] }
    | { type: "session_update"; sessionId: string; update: Record<string, unknown> }
    | { type: "saved_usage"; usage: ContextUsage }
    | { type: "turn_started" }
    | { type: "turn_completed"; stopReason?: string }
    | { type: "permission_requested"; request: AcpPermissionRequest }
    | { type: "permission_cleared"; requestId: string }
    | { type: "error"; message: string };

/* One run of a code fence that reads as one colour, and the lines they make up.
   A run with nothing set is plain text, which keeps the colour the stylesheet
   already gives the fence. */
export interface CodeToken {
    readonly text: string;
    readonly color?: string;
    readonly italic?: boolean;
    readonly bold?: boolean;
    readonly underline?: boolean;
}

export type CodeLine = readonly CodeToken[];

/** How full the session's context window is, as the agent last reported it. */
export interface ContextUsage {
    used: number;
    size: number;
    cost?: { amount: number; currency: string };
}
